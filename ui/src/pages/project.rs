use analytics_api::{ExceptionGroup, ExceptionStatus, Project as ProjectData, Stats};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::{
    ApiErrorAlert, Breakdown, Crumb, MetricCards, PageHeader, Range, RangePicker, TimeSeriesChart,
};
use crate::pages::ProjectSources;
use crate::search::{MatchContext, SearchContext, SearchVocabulary, VocabularyContext};

#[derive(Properties, PartialEq)]
pub struct ProjectProps {
    pub id: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Stats,
    Sources,
    Exceptions,
}

#[function_component(Project)]
pub fn project(props: &ProjectProps) -> Html {
    let id = props.id.clone();
    let project = use_state(|| None::<Result<ProjectData, ApiError>>);
    let tab = use_state(|| Tab::Stats);
    let range = use_state(Range::week);

    {
        let project = project.clone();
        let id = id.clone();
        use_effect_with(id.clone(), move |_| {
            spawn_local(async move {
                project.set(Some(api::get_project(&id).await));
            });
            || ()
        });
    }

    let name = match &*project {
        Some(Ok(p)) => p.name.clone(),
        _ => "Project".to_string(),
    };

    let tab_button = |this: Tab, label: &str| {
        let tab = tab.clone();
        let active = *tab == this;
        let onclick = Callback::from(move |_| tab.set(this));
        html! {
            <button class={classes!("tab", active.then_some("tab--active"))} {onclick}>{ label }</button>
        }
    };

    // The lookback applies to the statistics and exceptions tabs.
    let show_range = !matches!(*tab, Tab::Sources);
    let set_range = {
        let range = range.clone();
        Callback::from(move |r: Range| range.set(r))
    };

    html! {
        <div class="page">
            <PageHeader
                crumbs={vec![Crumb::link("Overview", Route::Overview), Crumb::current(name.clone())]}
                title={name.clone()}
            >
                if show_range {
                    <RangePicker value={*range} on_change={set_range} />
                }
            </PageHeader>
            if let Some(Err(err)) = &*project {
                <ApiErrorAlert error={err.clone()} />
            }
            <div class="tabs">
                { tab_button(Tab::Stats, "Statistics") }
                { tab_button(Tab::Sources, "Sources") }
                { tab_button(Tab::Exceptions, "Exceptions") }
            </div>
            {
                match *tab {
                    Tab::Stats => html! { <ProjectStats id={id.clone()} range={*range} /> },
                    Tab::Sources => html! { <ProjectSources id={id.clone()} /> },
                    Tab::Exceptions => html! { <ProjectExceptions id={id.clone()} range={*range} /> },
                }
            }
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct TabProps {
    id: String,
    range: Range,
}

#[function_component(ProjectStats)]
fn project_stats(props: &TabProps) -> Html {
    let id = props.id.clone();
    let range = props.range;
    let stats = use_state(|| None::<Result<Stats, ApiError>>);
    let vocabulary = use_context::<VocabularyContext>();
    {
        let stats = stats.clone();
        use_effect_with((id.clone(), range), move |(id, range)| {
            let (id, query) = (id.clone(), range.query());
            spawn_local(async move {
                stats.set(Some(api::project_stats(&id, &query).await));
            });
            || ()
        });
    }

    // Publish page/source names so the app-bar can complete `page:` / `source:`.
    let pages_vocab: Vec<AttrValue> = match &*stats {
        Some(Ok(s)) => s.pages.iter().map(|k| AttrValue::from(k.key.clone())).collect(),
        _ => Vec::new(),
    };
    let sources_vocab: Vec<AttrValue> = match &*stats {
        Some(Ok(s)) => s.sources.iter().map(|k| AttrValue::from(k.key.clone())).collect(),
        _ => Vec::new(),
    };
    {
        let vocabulary = vocabulary.clone();
        let (pv, sv) = (pages_vocab.clone(), sources_vocab.clone());
        use_effect_with((pv.clone(), sv.clone()), move |_| {
            if let Some(vocabulary) = &vocabulary {
                vocabulary.set.emit(SearchVocabulary { pages: pv, sources: sv, statuses: Vec::new() });
            }
            // Clear our contribution on unmount so its completions don't leak to
            // pages that publish no vocabulary (e.g. the overview).
            let cleanup = vocabulary.clone();
            move || {
                if let Some(cleanup) = &cleanup {
                    cleanup.set.emit(SearchVocabulary::default());
                }
            }
        });
    }

    match &*stats {
        None => html! { <p class="muted">{ "Loading…" }</p> },
        Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
        Some(Ok(s)) => html! {
            <>
                <MetricCards summary={s.summary.clone()} />
                <div class="panel"><TimeSeriesChart points={s.timeseries.clone()} /></div>
                <div class="breakdowns">
                    <Breakdown title="Top pages" rows={s.pages.clone()} />
                    <Breakdown title="Referrers" rows={s.referrers.clone()} />
                    <Breakdown title="Sources" rows={s.sources.clone()} />
                    <Breakdown title="Browsers" rows={s.browsers.clone()} />
                    <Breakdown title="Operating systems" rows={s.operating_systems.clone()} />
                    <Breakdown title="Devices" rows={s.devices.clone()} />
                    <Breakdown title="Countries" rows={s.countries.clone()} />
                    <Breakdown title="Languages" rows={s.languages.clone()} />
                </div>
            </>
        },
    }
}

#[function_component(ProjectExceptions)]
fn project_exceptions(props: &TabProps) -> Html {
    let id = props.id.clone();
    let range = props.range;
    let groups = use_state(|| None::<Result<Vec<ExceptionGroup>, ApiError>>);
    let vocabulary = use_context::<VocabularyContext>();
    let filter = use_context::<SearchContext>()
        .map(|s| s.filter.clone())
        .unwrap_or_default();
    {
        let groups = groups.clone();
        use_effect_with((id.clone(), range), move |(id, range)| {
            let (id, query) = (id.clone(), range.query());
            spawn_local(async move {
                groups.set(Some(api::list_exceptions(&id, &query).await));
            });
            || ()
        });
    }

    // Offer the exception statuses for `status:` completion.
    {
        let vocabulary = vocabulary.clone();
        use_effect_with((), move |_| {
            if let Some(vocabulary) = &vocabulary {
                vocabulary.set.emit(SearchVocabulary {
                    statuses: vec!["unresolved".into(), "resolved".into(), "ignored".into()],
                    ..Default::default()
                });
            }
            let cleanup = vocabulary.clone();
            move || {
                if let Some(cleanup) = &cleanup {
                    cleanup.set.emit(SearchVocabulary::default());
                }
            }
        });
    }

    match &*groups {
        None => html! { <p class="muted">{ "Loading…" }</p> },
        Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
        Some(Ok(list)) if list.is_empty() => {
            html! { <div class="empty">{ "No exceptions reported." }</div> }
        }
        Some(Ok(list)) => {
            let filtered: Vec<_> = list
                .iter()
                .filter(|g| {
                    let status = status_label(g.status).to_lowercase();
                    let text = format!("{} {} {}", g.exc_type, g.sample_message, status).to_lowercase();
                    filter.matches(&MatchContext { status: &status, text: &text, ..Default::default() })
                })
                .collect();
            let rows = filtered.iter().map(|g| html! {
                <tr>
                    <td>
                        <Link<Route> to={Route::Exception { project: id.clone(), group: g.group_id.clone() }}>
                            { &g.exc_type }
                        </Link<Route>>
                    </td>
                    <td class="ellipsis" title={g.sample_message.clone()}>{ &g.sample_message }</td>
                    <td>{ g.count }</td>
                    <td><span class={status_class(g.status)}>{ status_label(g.status) }</span></td>
                </tr>
            }).collect::<Html>();

            if filtered.is_empty() {
                html! { <div class="empty">{ "No exceptions match your search." }</div> }
            } else {
                html! {
                    <div class="card-table">
                        <table class="list">
                            <thead><tr><th>{ "Type" }</th><th>{ "Message" }</th><th>{ "Count" }</th><th>{ "Status" }</th></tr></thead>
                            <tbody>{ rows }</tbody>
                        </table>
                    </div>
                }
            }
        }
    }
}

pub fn status_label(status: ExceptionStatus) -> &'static str {
    match status {
        ExceptionStatus::Unresolved => "Unresolved",
        ExceptionStatus::Resolved => "Resolved",
        ExceptionStatus::Ignored => "Ignored",
    }
}

pub fn status_class(status: ExceptionStatus) -> Classes {
    let modifier = match status {
        ExceptionStatus::Unresolved => "badge--warn",
        ExceptionStatus::Resolved => "badge--ok",
        ExceptionStatus::Ignored => "badge--muted",
    };
    classes!("badge", modifier)
}
