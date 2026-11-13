//! The global Exceptions inbox: recent crashes/failures across every project
//! (and unassigned sources), for triage and investigation. Shares the URL
//! filter state with the dashboard — project/source/browser/OS/device chips
//! apply here; other dimensions render inert since exception events don't
//! carry them.

use std::cell::RefCell;
use std::rc::Rc;

use analytics_api::{ExceptionStatus, GlobalException};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yewi::prelude::{
    EmptyState, PageHeader, Pill, Tab, Tabs, Text, TextColor, TextInput, TextInputKind, TextSize,
    Tone, attrs, icons,
};

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::page::loading;
use crate::components::status::status_pill;
use crate::components::{
    ApiErrorAlert, FilterBar, ProjectsContext, RangeSelect, Sparkline, SuggestOption,
};
use crate::filters::{Dim, use_filters, use_navigate_with_query};
use crate::format::{ago, group_thousands};

thread_local! {
    /// The last-used status tab and text filter, so a detail round-trip (or any
    /// navigation) restores the inbox the way the operator left it.
    static INBOX_VIEW: RefCell<(StatusTab, String)> =
        const { RefCell::new((StatusTab::Unresolved, String::new())) };
}

/// The status tabs across the top of the inbox.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum StatusTab {
    #[default]
    Unresolved,
    Resolved,
    Ignored,
    All,
}

impl StatusTab {
    const ALL: [StatusTab; 4] = [
        StatusTab::Unresolved,
        StatusTab::Resolved,
        StatusTab::Ignored,
        StatusTab::All,
    ];

    /// The tab's value in `Tabs`.
    fn token(self) -> &'static str {
        match self {
            StatusTab::Unresolved => "unresolved",
            StatusTab::Resolved => "resolved",
            StatusTab::Ignored => "muted",
            StatusTab::All => "all",
        }
    }

    fn label(self) -> &'static str {
        match self {
            StatusTab::Unresolved => "Unresolved",
            StatusTab::Resolved => "Resolved",
            StatusTab::Ignored => "Muted",
            StatusTab::All => "All",
        }
    }

    fn matches(self, status: ExceptionStatus) -> bool {
        match self {
            StatusTab::Unresolved => status == ExceptionStatus::Unresolved,
            StatusTab::Resolved => status == ExceptionStatus::Resolved,
            StatusTab::Ignored => status == ExceptionStatus::Ignored,
            StatusTab::All => true,
        }
    }
}

#[function_component(Exceptions)]
pub fn exceptions() -> Html {
    let filters = use_filters();
    let projects = use_context::<ProjectsContext>()
        .map(|c| c.projects.clone())
        .unwrap_or_default();
    let data = use_state(|| None::<Result<Vec<GlobalException>, ApiError>>);
    let tab = use_state(|| INBOX_VIEW.with(|v| v.borrow().0));
    let needle = use_state(|| INBOX_VIEW.with(|v| v.borrow().1.clone()));
    // Only the latest request may publish (out-of-order responses would show
    // data that disagrees with the active chips).
    let fetch_seq = use_mut_ref(|| 0u64);

    {
        let (data, fetch_seq) = (data.clone(), fetch_seq.clone());
        let filters = filters.clone();
        use_effect_with(filters.canonical(), move |_| {
            let seq = {
                let mut current = fetch_seq.borrow_mut();
                *current += 1;
                *current
            };
            let query = filters.exceptions_query(js_sys::Date::now() as i64);
            spawn_local(async move {
                let result = api::list_all_exceptions(&query).await;
                if *fetch_seq.borrow() == seq {
                    data.set(Some(result));
                }
            });
            || ()
        });
    }

    let on_needle = {
        let needle = needle.clone();
        Callback::from(move |value: String| {
            INBOX_VIEW.with(|v| v.borrow_mut().1 = value.clone());
            needle.set(value);
        })
    };

    // Suggestions for the (restricted) filter chips on this page: the full
    // project list (filters address projects by name) and the sources seen in
    // the current listing.
    let suggestions: Vec<(Dim, Vec<SuggestOption>)> = {
        let sources: Vec<SuggestOption> = match &*data {
            Some(Ok(list)) => {
                let mut seen: Vec<String> = Vec::new();
                for e in list {
                    if !seen.contains(&e.source) {
                        seen.push(e.source.clone());
                    }
                }
                seen.into_iter()
                    .map(|uri| SuggestOption {
                        label: analytics_api::source_label(&uri).to_string(),
                        value: uri,
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        let project_options: Vec<SuggestOption> = projects
            .iter()
            .map(|p| SuggestOption {
                value: p.name.clone(),
                label: p.name.clone(),
            })
            .collect();
        vec![(Dim::Project, project_options), (Dim::Source, sources)]
    };

    let body = match &*data {
        None => loading(),
        Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
        Some(Ok(list)) => {
            let text = needle.to_lowercase();
            let mut counts = [0usize; 4];
            for e in list {
                for (i, t) in StatusTab::ALL.into_iter().enumerate() {
                    if t.matches(e.group.status) {
                        counts[i] += 1;
                    }
                }
            }

            let visible: Vec<&GlobalException> = list
                .iter()
                .filter(|e| tab.matches(e.group.status))
                .filter(|e| {
                    text.is_empty()
                        || e.group.exc_type.to_lowercase().contains(&text)
                        || e.group.sample_message.to_lowercase().contains(&text)
                        || e.project_name
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&text)
                })
                .collect();

            // The list is the page's content, so it sits flat under the tabs
            // with a rule between groups.
            let list = if visible.is_empty() {
                html! {
                    <EmptyState icon={icons::SEARCH_X} title={
                        if list.is_empty() { "No exceptions reported in this period." }
                        else { "No exceptions match the current filters." }
                    } />
                }
            } else {
                html! { <div class="exc-list">{ for visible.iter().map(|e| exception_row(e)) }</div> }
            };
            let tabs = StatusTab::ALL.into_iter().enumerate().map(|(i, t)| {
                html_nested! {
                    <Tab key={t.token()} value={t.token()} label={t.label()} count={counts[i].to_string()}>
                        if *tab == t {
                            { list.clone() }
                        }
                    </Tab>
                }
            });
            let on_change = {
                let tab = tab.clone();
                Callback::from(move |value: AttrValue| {
                    if let Some(t) = StatusTab::ALL
                        .into_iter()
                        .find(|t| t.token() == value.as_str())
                    {
                        INBOX_VIEW.with(|v| v.borrow_mut().0 = t);
                        tab.set(t);
                    }
                })
            };

            html! {
                <Tabs selected={tab.token()} {on_change} label="Triage status">{ for tabs }</Tabs>
            }
        }
    };

    let actions = html! {
        <>
            <TextInput kind={TextInputKind::Search} placeholder="Filter by type, message, project…"
                value={(*needle).clone()} on_input={on_needle} class="page-search"
                attrs={attrs! { "aria-label" => "Filter exceptions" }} />
            <RangeSelect />
        </>
    };

    html! {
        <div class="page">
            <PageHeader title="Exceptions" subtitle="Crashes and errors, grouped by fingerprint." {actions} />
            <FilterBar suggestions={Rc::new(suggestions)} restricted={true} />
            { body }
        </div>
    }
}

fn exception_row(e: &GlobalException) -> Html {
    let row_body = html! {
        <>
            <div class="exc-row__main">
                <div class="exc-row__title">
                    <span class="exc-row__type">{ &e.group.exc_type }</span>
                    <span class="exc-row__message" title={e.group.sample_message.clone()}>
                        { &e.group.sample_message }
                    </span>
                </div>
                <div class="exc-row__meta">
                    { status_pill(e.group.status) }
                    {
                        // Groups are per source, so the row names the application
                        // it failed on alongside its owning project.
                        match &e.project_name {
                            Some(name) => html! {
                                <>
                                    <span>{ name.clone() }</span>
                                    <code title={e.source.clone()}>{ analytics_api::source_label(&e.source) }</code>
                                </>
                            },
                            None => html! {
                                <Pill dot=false attrs={attrs! { "title" => "This source is not assigned to a project. Assign it to enable triage and detail." }}>
                                    { format!("Unassigned · {}", analytics_api::source_label(&e.source)) }
                                </Pill>
                            },
                        }
                    }
                    <span>{ format!("first seen {}", ago(e.group.first_seen_ms)) }</span>
                </div>
            </div>
            <Sparkline points={e.group.trend.clone()} tone={Tone::Danger} class={classes!("exc-row__trend")} />
            <div class="exc-row__count">
                <Text weight={yewi::TextWeight::Semibold} tabular=true>{ group_thousands(e.group.count) }</Text>
                <Text size={TextSize::Sm} color={TextColor::Subtle}>{ ago(e.group.last_seen_ms) }</Text>
            </div>
        </>
    };

    // Only groups attributed to a project have a detail page (triage is keyed
    // by project); unassigned rows render inert with an explanatory badge.
    match &e.project_id {
        Some(project) => html! {
            <ExceptionLink
                project={project.clone()}
                group={e.group.group_id.clone()}
                source={e.source.clone()}
            >
                { row_body }
            </ExceptionLink>
        },
        None => html! { <div class="exc-row exc-row--inert">{ row_body }</div> },
    }
}

#[derive(Properties, PartialEq)]
struct ExceptionLinkProps {
    project: String,
    group: String,
    source: String,
    children: Html,
}

/// A row that navigates to the exception detail page, carrying the current
/// filter state (so returning to the inbox restores it) plus the row's source
/// (a group's identity is fingerprint + the application it was seen on).
/// Rendered as a role="button" div (not a `<button>`) because the row contains
/// flow content and badges, which the button content model forbids.
#[function_component(ExceptionLink)]
fn exception_link(props: &ExceptionLinkProps) -> Html {
    let filters = use_filters();
    let navigate = use_navigate_with_query();
    let go = {
        let (project, group, source) = (
            props.project.clone(),
            props.group.clone(),
            props.source.clone(),
        );
        move || {
            let mut pairs = filters.to_pairs();
            pairs.push(("source".into(), source.clone()));
            navigate.emit((
                Route::Exception {
                    project: project.clone(),
                    group: group.clone(),
                },
                pairs,
            ));
        }
    };
    let onclick = {
        let go = go.clone();
        Callback::from(move |_: MouseEvent| go())
    };
    let onkeydown = Callback::from(move |e: KeyboardEvent| {
        if matches!(e.key().as_str(), "Enter" | " ") {
            e.prevent_default();
            go();
        }
    });
    html! {
        <div class="exc-row" role="button" tabindex="0" onclick={onclick} onkeydown={onkeydown}>
            { props.children.clone() }
        </div>
    }
}
