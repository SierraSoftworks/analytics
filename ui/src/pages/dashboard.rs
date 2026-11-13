//! The global dashboard: every project's traffic in one filterable view.
//! Clicking any breakdown row adds a filter chip (clicking it again removes
//! it); the URL carries the whole filter state, so every drill-down is
//! shareable and back-button friendly.

use std::rc::Rc;

use analytics_api::{Dashboard as DashboardData, SourceInput, source_label};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yewi::prelude::{
    Button, ButtonVariant, Card, Heading, HeadingLevel, PageHeader, Select, SelectOption, Size,
    Text, TextColor, TextSize, ToggleSwitch,
};

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::charts::Metric;
use crate::components::page::loading;
use crate::components::{
    ActionIcon, ApiErrorAlert, BreakdownPanel, FilterBar, MetricCards, PanelRow, PanelTab,
    ProjectDrawer, ProjectsContext, RangeSelect, SuggestOption, TimeSeriesChart, TraceList,
};
use crate::filters::{Dim, TimeRange, use_apply_filters, use_filters, use_navigate_with_query};
use crate::format::{country_flag, country_name, group_thousands, language_name};

#[function_component(Dashboard)]
pub fn dashboard() -> Html {
    let filters = use_filters();
    let apply = use_apply_filters();
    let projects_ctx = use_context::<ProjectsContext>();

    let data = use_state(|| None::<Result<DashboardData, ApiError>>);
    let refreshing = use_state(|| false);
    let reload = use_state(|| 0u32);
    let metric = use_state(Metric::default);
    let compare = use_state(|| true);
    let manage_project = use_state(|| None::<String>);
    // Monotonic fetch sequence: only the *latest* request may publish its
    // response, so a slow query can't overwrite a faster later one and show
    // data that disagrees with the URL.
    let fetch_seq = use_mut_ref(|| 0u64);

    // Fetch whenever the (canonical) filter state changes. Presets resolve to
    // absolute timestamps here — inside the effect — so re-renders never shift
    // the window and re-trigger the fetch.
    {
        let (data, refreshing, fetch_seq) = (data.clone(), refreshing.clone(), fetch_seq.clone());
        let filters = filters.clone();
        use_effect_with((filters.canonical(), *reload), move |_| {
            let seq = {
                let mut current = fetch_seq.borrow_mut();
                *current += 1;
                *current
            };
            refreshing.set(true);
            let query = filters.stats_query(js_sys::Date::now() as i64);
            spawn_local(async move {
                let result = api::dashboard(&query).await;
                if *fetch_seq.borrow() == seq {
                    data.set(Some(result));
                    refreshing.set(false);
                }
            });
            || ()
        });
    }

    // Toggle semantics: clicking a row whose terms (most rows carry one; a
    // version row also pins its application) are all already active clears
    // them instead of re-applying them.
    let on_filter = {
        let (apply, filters) = (apply.clone(), filters.clone());
        Callback::from(move |terms: Vec<(Dim, String)>| {
            let active = terms
                .iter()
                .all(|(dim, value)| filters.get(*dim) == Some(value.as_str()));
            let mut next = filters.clone();
            for (dim, value) in terms {
                next = if active {
                    next.without(dim)
                } else {
                    next.with(dim, value)
                };
            }
            apply.emit(next);
        })
    };

    let on_zoom = {
        let (apply, filters) = (apply.clone(), filters.clone());
        Callback::from(move |(from, to): (i64, i64)| {
            apply.emit(filters.with_range(TimeRange::Custom { from, to }));
        })
    };

    let on_metric = {
        let metric = metric.clone();
        Callback::from(move |m: Metric| metric.set(m))
    };
    let set_compare = {
        let compare = compare.clone();
        Callback::from(move |on: bool| compare.set(on))
    };

    let projects = projects_ctx
        .as_ref()
        .map(|c| c.projects.clone())
        .unwrap_or_default();
    // Filter values address projects by their (unique) name; breakdown rows
    // arrive keyed by id. Resolve either to the canonical display name.
    let project_name = {
        let projects = projects.clone();
        move |value: &str| -> String {
            let needle = value.to_lowercase();
            projects
                .iter()
                .find(|p| p.name.to_lowercase() == needle || p.id == value)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| value.to_string())
        }
    };

    let on_manage = {
        let manage_project = manage_project.clone();
        let projects = projects.clone();
        // Rows carry the project *name* (the filter value); the drawer's API
        // calls need the id.
        Callback::from(move |value: String| {
            let id = projects
                .iter()
                .find(|p| p.name == value || p.id == value)
                .map(|p| p.id.clone())
                .unwrap_or(value);
            manage_project.set(Some(id));
        })
    };
    let close_manage = {
        let manage_project = manage_project.clone();
        Callback::from(move |_: ()| manage_project.set(None))
    };
    // Open an event's detail view, carrying the filter state so its numbers
    // cover the same slice as the row that was clicked.
    let open_event = {
        let navigate = use_navigate_with_query();
        let filters = filters.clone();
        Callback::from(move |name: String| {
            let mut pairs = filters.to_pairs();
            pairs.push(("name".into(), name));
            navigate.emit((Route::Event, pairs));
        })
    };
    let bump = {
        let reload = reload.clone();
        Callback::from(move |_: ()| reload.set(*reload + 1))
    };

    // The filter bar renders even while loading or on error — losing the
    // controls that change the query would strand the operator on a bad state.
    let suggestions = match &*data {
        Some(Ok(dash)) => build_suggestions(dash, &projects),
        _ => vec![(
            Dim::Project,
            projects
                .iter()
                .map(|p| SuggestOption {
                    value: p.name.clone(),
                    label: p.name.clone(),
                })
                .collect(),
        )],
    };

    let body = match &*data {
        None => loading(),
        Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
        Some(Ok(dash)) => {
            let active = filters.query.terms.clone();

            // ---------------------------------------------------- panel rows
            let plain = |rows: &[analytics_api::BreakdownRow], absent: &str| -> Vec<PanelRow> {
                rows.iter()
                    .map(|r| PanelRow {
                        value: r.key.clone(),
                        label: if r.key.is_empty() {
                            absent.to_string()
                        } else {
                            r.key.clone()
                        },
                        icon: None,
                        visitors: r.visitors,
                        pageviews: r.pageviews,
                        events: r.events,
                        extra: None,
                    })
                    .collect()
            };
            let versions = dash
                .breakdowns
                .versions
                .iter()
                .map(|r| PanelRow {
                    value: r.version.clone(),
                    label: version_label(r),
                    icon: None,
                    visitors: r.visitors,
                    pageviews: r.pageviews,
                    events: r.events,
                    // The version alone would also match other applications'
                    // releases, so clicking pins the application with it.
                    extra: Some((Dim::Browser, r.app.clone())),
                })
                .collect::<Vec<_>>();
            let countries = dash
                .breakdowns
                .countries
                .iter()
                .map(|r| PanelRow {
                    value: r.key.clone(),
                    label: if r.key.is_empty() {
                        Dim::Country.absent_label().to_string()
                    } else {
                        country_name(&r.key)
                    },
                    icon: country_flag(&r.key),
                    visitors: r.visitors,
                    pageviews: r.pageviews,
                    events: r.events,
                    extra: None,
                })
                .collect::<Vec<_>>();
            let languages = dash
                .breakdowns
                .languages
                .iter()
                .map(|r| PanelRow {
                    value: r.key.clone(),
                    label: if r.key.is_empty() {
                        Dim::Language.absent_label().to_string()
                    } else {
                        language_name(&r.key)
                    },
                    icon: None,
                    visitors: r.visitors,
                    pageviews: r.pageviews,
                    events: r.events,
                    extra: None,
                })
                .collect::<Vec<_>>();
            // Project rows filter (and highlight) by name — the filter value —
            // not by the id the API keys them with.
            let project_rows = dash
                .breakdowns
                .projects
                .iter()
                .map(|r| PanelRow {
                    value: project_name(&r.key),
                    label: project_name(&r.key),
                    icon: None,
                    visitors: r.visitors,
                    pageviews: r.pageviews,
                    events: r.events,
                    extra: None,
                })
                .collect::<Vec<_>>();
            let source_rows = dash
                .breakdowns
                .sources
                .iter()
                .map(|r| PanelRow {
                    value: r.key.clone(),
                    label: source_label(&r.key).to_string(),
                    icon: None,
                    visitors: r.visitors,
                    pageviews: r.pageviews,
                    events: r.events,
                    extra: None,
                })
                .collect::<Vec<_>>();

            // Sources lead the grid: for a single-org deployment "which
            // site/app is this?" is the first drill-down an operator reaches
            // for. Projects (which merely group sources) sit last.
            let source_tabs = vec![PanelTab::new("Sources", Dim::Source, source_rows)];
            let pages_tabs = vec![
                PanelTab::new(
                    "Top pages",
                    Dim::Path,
                    plain(&dash.breakdowns.pages, "Unknown"),
                ),
                PanelTab::new(
                    "Events",
                    Dim::EventName,
                    plain(&dash.breakdowns.event_names, Dim::EventName.absent_label()),
                )
                .with_action("View event details", open_event.clone())
                .with_action_icon(ActionIcon::Open),
            ];
            let acquisition_tabs = vec![
                PanelTab::new(
                    "Referrers",
                    Dim::Referrer,
                    plain(&dash.breakdowns.referrers, Dim::Referrer.absent_label()),
                ),
                PanelTab::new(
                    "UTM source",
                    Dim::UtmSource,
                    plain(&dash.breakdowns.utm_sources, "None"),
                ),
                PanelTab::new(
                    "UTM medium",
                    Dim::UtmMedium,
                    plain(&dash.breakdowns.utm_mediums, "None"),
                ),
                PanelTab::new(
                    "UTM campaign",
                    Dim::UtmCampaign,
                    plain(&dash.breakdowns.utm_campaigns, "None"),
                ),
            ];
            let location_tabs = vec![
                PanelTab::new("Countries", Dim::Country, countries),
                PanelTab::new("Languages", Dim::Language, languages),
            ];
            let platform_tabs = vec![
                PanelTab::new(
                    "Applications",
                    Dim::Browser,
                    plain(&dash.breakdowns.browsers, "Unknown"),
                ),
                PanelTab::new("Versions", Dim::Version, versions),
                PanelTab::new(
                    "OS",
                    Dim::Os,
                    plain(&dash.breakdowns.operating_systems, "Unknown"),
                ),
                PanelTab::new(
                    "Devices",
                    Dim::Device,
                    plain(&dash.breakdowns.devices, "Unknown"),
                ),
            ];
            let project_tabs = vec![
                PanelTab::new("Projects", Dim::Project, project_rows)
                    .with_action("Manage project", on_manage.clone()),
            ];

            html! {
                <div class={classes!("dashboard", refreshing.then_some("dashboard--refreshing"))}>
                    <UnassignedBanner dash={dash.clone()} on_changed={bump.clone()} />
                    <MetricCards
                        summary={dash.summary.clone()}
                        previous={dash.previous_summary.clone()}
                        metric={*metric}
                        on_metric={on_metric.clone()}
                    />
                    <section class="section">
                        <header class="section__head">
                            <Heading level={HeadingLevel::H2} size={TextSize::Md}>{ metric.label() }</Heading>
                            <div class="section__actions">
                                <Text size={TextSize::Sm} color={TextColor::Subtle}>{ "Drag to zoom" }</Text>
                                <ToggleSwitch checked={*compare} on_change={set_compare.clone()} size={Size::Small}>
                                    { "Compare" }
                                </ToggleSwitch>
                            </div>
                        </header>
                        <TimeSeriesChart
                            points={dash.timeseries.clone()}
                            previous={dash.previous_timeseries.clone()}
                            metric={*metric}
                            compare={*compare}
                            on_zoom={Some(on_zoom.clone())}
                        />
                    </section>
                    <div class="panel-grid">
                        <BreakdownPanel tabs={source_tabs} metric={*metric} on_filter={on_filter.clone()} active={active.clone()} />
                        // Follows the metric: Events mode surfaces the event-name
                        // histogram, the view metrics bring Top pages back.
                        <BreakdownPanel tabs={pages_tabs} metric={*metric} on_filter={on_filter.clone()} active={active.clone()}
                            select_tab={Some(if *metric == Metric::Events { 1 } else { 0 })} />
                        <BreakdownPanel tabs={acquisition_tabs} metric={*metric} on_filter={on_filter.clone()} active={active.clone()} />
                        <BreakdownPanel tabs={location_tabs} metric={*metric} on_filter={on_filter.clone()} active={active.clone()} />
                        <BreakdownPanel tabs={platform_tabs} metric={*metric} on_filter={on_filter.clone()} active={active.clone()} />
                        <BreakdownPanel tabs={project_tabs} metric={*metric} on_filter={on_filter.clone()} active={active.clone()} />
                    </div>
                    <TraceList
                        traces={dash.traces.clone()}
                        hint="The most recent sessions matching the filters above"
                        empty="No session traces in this period — traces appear once visits report a session id."
                    />
                </div>
            }
        }
    };

    let project_filter = filters.get(Dim::Project).map(str::to_string);
    let heading = project_filter
        .as_deref()
        .map(&project_name)
        .unwrap_or_else(|| "Dashboard".to_string());
    html! {
        <div class="page">
            <PageHeader title={heading} subtitle="Click any value to filter by it."
                actions={html! { <RangeSelect /> }} />
            <FilterBar suggestions={Rc::new(suggestions)} />
            { body }
            <ProjectDrawer
                project_id={(*manage_project).clone()}
                on_close={close_manage}
                on_changed={bump}
            />
        </div>
    }
}

/// The add-filter popover's value suggestions, sourced from the current
/// breakdown rows (top values) plus the full project list.
fn build_suggestions(
    dash: &DashboardData,
    projects: &Rc<Vec<analytics_api::Project>>,
) -> Vec<(Dim, Vec<SuggestOption>)> {
    let rows = |rows: &[analytics_api::BreakdownRow], absent: &str| -> Vec<SuggestOption> {
        rows.iter()
            .map(|r| SuggestOption {
                value: r.key.clone(),
                label: if r.key.is_empty() {
                    absent.to_string()
                } else {
                    r.key.clone()
                },
            })
            .collect()
    };
    vec![
        (
            Dim::Project,
            projects
                .iter()
                .map(|p| SuggestOption {
                    value: p.name.clone(),
                    label: p.name.clone(),
                })
                .collect(),
        ),
        (
            Dim::Source,
            dash.breakdowns
                .sources
                .iter()
                .map(|r| SuggestOption {
                    value: r.key.clone(),
                    label: source_label(&r.key).to_string(),
                })
                .collect(),
        ),
        (Dim::Path, rows(&dash.breakdowns.pages, "Unknown")),
        (
            Dim::EventName,
            rows(&dash.breakdowns.event_names, Dim::EventName.absent_label()),
        ),
        (
            Dim::Referrer,
            rows(&dash.breakdowns.referrers, Dim::Referrer.absent_label()),
        ),
        (
            Dim::Country,
            dash.breakdowns
                .countries
                .iter()
                .map(|r| SuggestOption {
                    value: r.key.clone(),
                    label: if r.key.is_empty() {
                        Dim::Country.absent_label().to_string()
                    } else {
                        match country_flag(&r.key) {
                            Some(flag) => format!("{flag} {}", country_name(&r.key)),
                            None => country_name(&r.key),
                        }
                    },
                })
                .collect(),
        ),
        (
            Dim::Language,
            dash.breakdowns
                .languages
                .iter()
                .map(|r| SuggestOption {
                    value: r.key.clone(),
                    label: if r.key.is_empty() {
                        Dim::Language.absent_label().to_string()
                    } else {
                        language_name(&r.key)
                    },
                })
                .collect(),
        ),
        (Dim::Browser, rows(&dash.breakdowns.browsers, "Unknown")),
        (
            Dim::Version,
            dash.breakdowns
                .versions
                .iter()
                .map(|r| SuggestOption {
                    value: r.version.clone(),
                    label: version_label(r),
                })
                .collect(),
        ),
        (Dim::Os, rows(&dash.breakdowns.operating_systems, "Unknown")),
        (Dim::Device, rows(&dash.breakdowns.devices, "Unknown")),
        (Dim::UtmSource, rows(&dash.breakdowns.utm_sources, "None")),
        (Dim::UtmMedium, rows(&dash.breakdowns.utm_mediums, "None")),
        (
            Dim::UtmCampaign,
            rows(&dash.breakdowns.utm_campaigns, "None"),
        ),
    ]
}

/// The display form of a client-version row: qualified by its application,
/// since a bare version number is meaningless across applications. Falls back
/// to whichever half is known when the other is absent.
fn version_label(row: &analytics_api::VersionRow) -> String {
    match (row.app.is_empty(), row.version.is_empty()) {
        (false, false) => format!("{} @ {}", row.app, row.version),
        (false, true) => row.app.clone(),
        (true, false) => row.version.clone(),
        (true, true) => "Unknown".to_string(),
    }
}

#[derive(Properties, PartialEq)]
struct UnassignedBannerProps {
    dash: DashboardData,
    on_changed: Callback<()>,
}

/// A compact operator inbox for sources that belong to no project: the top few
/// by traffic with an inline assign control. Persistent while unassigned
/// sources exist (new sources must not be missable), gone once they're sorted.
#[function_component(UnassignedBanner)]
fn unassigned_banner(props: &UnassignedBannerProps) -> Html {
    let expanded = use_state(|| false);
    let projects_ctx = use_context::<ProjectsContext>();
    let projects = projects_ctx
        .as_ref()
        .map(|c| c.projects.clone())
        .unwrap_or_default();

    let unassigned = &props.dash.unassigned;
    if unassigned.is_empty() {
        return html! {};
    }

    let total_views: i64 = unassigned.iter().map(|r| r.pageviews + r.events).sum();
    let shown = if *expanded {
        unassigned.len()
    } else {
        unassigned.len().min(3)
    };

    let assign = {
        let (on_changed, reload) = (
            props.on_changed.clone(),
            projects_ctx.as_ref().map(|c| c.reload.clone()),
        );
        Callback::from(move |(uri, project_id): (String, String)| {
            let (on_changed, reload) = (on_changed.clone(), reload.clone());
            spawn_local(async move {
                let input = SourceInput {
                    project_id: Some(project_id),
                    ..Default::default()
                };
                if api::update_source(&uri, &input).await.is_ok() {
                    on_changed.emit(());
                    if let Some(reload) = &reload {
                        reload.emit(());
                    }
                }
            });
        })
    };
    let open_new = {
        let open_new = projects_ctx.as_ref().map(|c| c.open_new.clone());
        Callback::from(move |_: MouseEvent| {
            if let Some(open_new) = &open_new {
                open_new.emit(());
            }
        })
    };

    let options: Vec<SelectOption> = projects
        .iter()
        .map(|p| SelectOption::new(p.id.clone(), p.name.clone()))
        .collect();
    let rows = unassigned.iter().take(shown).map(|row| {
        let on_change = {
            let (assign, uri) = (assign.clone(), row.key.clone());
            Callback::from(move |project_id: AttrValue| assign.emit((uri.clone(), project_id.to_string())))
        };
        html! {
            <li class="unassigned__row" key={row.key.clone()}>
                <code class="unassigned__source" title={row.key.clone()}>{ source_label(&row.key) }</code>
                <Text size={TextSize::Sm} color={TextColor::Subtle} tabular=true>
                    { format!("{} views · {} visitors", group_thousands(row.pageviews + row.events), group_thousands(row.visitors)) }
                </Text>
                <Select options={options.clone()} placeholder="Assign to project…" {on_change}
                    size={Size::Small} fit=true label={format!("Project for {}", source_label(&row.key))} />
            </li>
        }
    });

    let more = unassigned.len().saturating_sub(shown);
    let toggle = {
        let expanded = expanded.clone();
        Callback::from(move |_: MouseEvent| expanded.set(!*expanded))
    };

    // A call-out set apart from the traffic below it: new sources must not be
    // missable.
    html! {
        <Card
            class="unassigned"
            title={format!("{} unassigned source{}", unassigned.len(), if unassigned.len() == 1 { "" } else { "s" })}
            subtitle={format!("{} views this period", group_thousands(total_views))}
            actions={html! { <Button size={Size::Small} on_click={open_new}>{ "New project" }</Button> }}
        >
            <ul class="unassigned__rows">{ for rows }</ul>
            if more > 0 {
                <Button variant={ButtonVariant::Subtle} size={Size::Small} on_click={toggle}>
                    { format!("Show {more} more") }
                </Button>
            } else if *expanded && unassigned.len() > 3 {
                <Button variant={ButtonVariant::Subtle} size={Size::Small} on_click={toggle}>
                    { "Show fewer" }
                </Button>
            }
        </Card>
    }
}
