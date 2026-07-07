//! One exception group in forensic detail: triage controls, the occurrence
//! trend, how failures distribute across key dimensions (app version, OS,
//! application, …), and a scrubber over the group's **distinct variants** — one
//! representative example per unique message/stack, with a count of the
//! occurrences it stands for.
//!
//! A group's identity is source-scoped — fingerprint + the application it was
//! seen on — carried as a `?source=` query parameter alongside the filter
//! state. Pre-source deep links (no `source`) fall back to the project-wide
//! aggregate.

use analytics_api::{
    ExceptionGroupDetail, ExceptionStatus, ExceptionVariant, TriageInput, source_label,
};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::use_location;

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::metadata::Metadata;
use crate::components::status::{status_class, status_label};
use crate::components::{
    ApiErrorAlert, Crumb, PageHeader, Sparkline, TraceList, distribution, icons,
};
use crate::filters::{query_param, use_filters, use_navigate_with_filters};
use crate::format::{ago, group_thousands, short_session_id};

#[derive(Properties, PartialEq)]
pub struct ExceptionDetailProps {
    pub project: String,
    pub group: String,
}

#[function_component(ExceptionDetail)]
pub fn exception_detail(props: &ExceptionDetailProps) -> Html {
    let (project, group) = (props.project.clone(), props.group.clone());
    let location = use_location();
    let source = location
        .as_ref()
        .and_then(|l| query_param(l.query_str(), "source"))
        .filter(|s| !s.trim().is_empty());
    let detail = use_state(|| None::<Result<ExceptionGroupDetail, ApiError>>);
    let reload = use_state(|| 0u32);
    // The row that linked here carried the inbox's filter state, so the detail
    // numbers cover the same window as the row the operator just read.
    let filters = use_filters();
    let range_label = filters.range_label();

    {
        let detail = detail.clone();
        let (project, group, source) = (project.clone(), group.clone(), source.clone());
        let filters = filters.clone();
        // `project` is a dependency: two projects can share a fingerprint, so
        // navigating between same-group exceptions across projects must refetch.
        use_effect_with(
            (project.clone(), group.clone(), filters.canonical(), *reload),
            move |_| {
                let range = filters.range_query(js_sys::Date::now() as i64);
                spawn_local(async move {
                    detail.set(Some(
                        api::exception_detail(&group, &project, source.as_deref(), &range).await,
                    ));
                });
                || ()
            },
        );
    }

    let set_status = {
        let (project, group, source, reload) = (
            project.clone(),
            group.clone(),
            source.clone(),
            reload.clone(),
        );
        move |status: ExceptionStatus| {
            let input = TriageInput {
                project_id: project.clone(),
                status,
                note: None,
                source: source.clone(),
            };
            let (group, reload) = (group.clone(), reload.clone());
            Callback::from(move |_: MouseEvent| {
                let (group, input, reload) = (group.clone(), input.clone(), reload.clone());
                spawn_local(async move {
                    if api::set_triage(&group, &input).await.is_ok() {
                        reload.set(*reload + 1);
                    }
                });
            })
        }
    };

    let title = match &*detail {
        Some(Ok(d)) => d.group.exc_type.clone(),
        _ => "Exception".to_string(),
    };
    let crumbs = vec![
        Crumb::link_with_query("Exceptions", Route::Exceptions, filters.to_pairs()),
        Crumb::current(title.clone()),
    ];

    html! {
        <div class="page">
            <PageHeader crumbs={crumbs} title={title} />
            {
                match &*detail {
                    None => html! { <div class="page-loading">{ "Loading…" }</div> },
                    Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
                    Some(Ok(detail)) => {
                        let status = detail.group.status;
                        // Colour + glyph carry the verb; the current state's
                        // action is a no-op and renders disabled.
                        let action = |target: ExceptionStatus, class: &'static str, label: &'static str, icon: Html| html! {
                            <button
                                class={classes!("exc-action", class)}
                                title={label}
                                aria-label={label}
                                disabled={status == target}
                                onclick={set_status(target)}
                            >
                                { icon }
                            </button>
                        };
                        let meta = format!(
                            "{} occurrences · first seen {} · last seen {}",
                            group_thousands(detail.group.count),
                            ago(detail.group.first_seen_ms),
                            ago(detail.group.last_seen_ms),
                        );
                        let meta = match &source {
                            Some(source) => format!("{} · {meta}", source_label(source)),
                            None => meta,
                        };
                        html! {
                        <>
                            <div class="exc-head panel">
                                <div class="exc-head__top">
                                    <span class={status_class(status)}>{ status_label(status) }</span>
                                    <span class="exc-head__title">
                                        <b class="exc-head__type">{ &detail.group.exc_type }</b>
                                        <code class="exc-head__message">{ &detail.group.sample_message }</code>
                                    </span>
                                    <div class="exc-head__actions">
                                        { action(ExceptionStatus::Resolved, "exc-action--resolve", "Mark resolved", icons::check()) }
                                        { action(ExceptionStatus::Ignored, "exc-action--ignore", "Ignore", icons::mute()) }
                                        { action(ExceptionStatus::Unresolved, "exc-action--reopen", "Reopen", icons::check_struck()) }
                                    </div>
                                </div>
                                <div class="exc-head__meta muted">{ meta }</div>
                                if !detail.group.trend.is_empty() {
                                    <div class="exc-head__trend">
                                        <span class="stat__label">{ range_label.clone() }</span>
                                        <Sparkline points={detail.group.trend.clone()} class={classes!("exc-head__spark")} />
                                    </div>
                                }
                            </div>

                            <h2 class="section__title">{ "Distribution" }</h2>
                            <div class="dist-grid">
                                { distribution("App versions", &detail.breakdowns.app_versions, detail.group.count) }
                                // Scoped to one source, the sources card is a
                                // single 100% row — the header names it instead.
                                if source.is_none() {
                                    { distribution("Apps / sources", &detail.breakdowns.sources, detail.group.count) }
                                }
                                { distribution("Applications", &detail.breakdowns.browsers, detail.group.count) }
                                { distribution("Operating systems", &detail.breakdowns.operating_systems, detail.group.count) }
                                { distribution("Devices", &detail.breakdowns.devices, detail.group.count) }
                            </div>

                            <VariantScrubber
                                variants={detail.variants.clone()}
                                scoped={source.is_some()}
                                key={detail.group.group_id.clone()}
                            />
                            // Pick which of the group's sessions to inspect.
                            <TraceList
                                traces={detail.traces.clone()}
                                hint="Sessions this exception occurred in"
                            />
                        </>
                        }
                    },
                }
            }
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct VariantScrubberProps {
    variants: Vec<ExceptionVariant>,
    /// Whether the view is scoped to one source — the application is then
    /// given, so releases read as bare version numbers.
    #[prop_or_default]
    scoped: bool,
}

/// Scrub through a group's distinct examples with ‹ › navigation; each shows
/// how many identical occurrences it represents.
#[function_component(VariantScrubber)]
fn variant_scrubber(props: &VariantScrubberProps) -> Html {
    let index = use_state(|| 0usize);
    let filters = use_filters();
    let navigate = use_navigate_with_filters();
    let count = props.variants.len();
    if count == 0 {
        return html! {};
    }
    let i = (*index).min(count - 1);
    let variant = &props.variants[i];

    let go = |delta: i64| {
        let index = index.clone();
        Callback::from(move |_: MouseEvent| {
            let next = (i as i64 + delta).rem_euclid(count as i64) as usize;
            index.set(next);
        })
    };

    let ua = match (&variant.ua_browser, &variant.ua_os) {
        (Some(b), Some(os)) => format!("{b} on {os}"),
        (Some(b), None) => b.clone(),
        (None, Some(os)) => os.clone(),
        _ => "unknown client".to_string(),
    };
    // The source is the application; pair it with the reported release. In a
    // source-scoped view the application is given, so the release stands alone.
    let app = if props.scoped {
        variant
            .app_version
            .as_ref()
            .map(|version| format!("v{version}"))
    } else {
        variant.source.as_ref().map(|source| {
            let label = source_label(source).to_string();
            match &variant.app_version {
                Some(version) => format!("{label} @ {version}"),
                None => label,
            }
        })
    };
    let metadata = Metadata::parse(variant.metadata.as_deref());

    // Jump to the session this exemplar occurred in, when it reported one.
    let trace_link = variant.session_id.as_ref().map(|sid| {
        let onclick = {
            let (navigate, filters) = (navigate.clone(), filters.clone());
            let id = sid.clone();
            Callback::from(move |_: MouseEvent| {
                navigate.emit((Route::Trace { id: id.clone() }, filters.clone()));
            })
        };
        html! {
            <button class="btn btn--small" onclick={onclick}
                title={format!("Open session {}", short_session_id(sid))}>
                { "View session trace" }
            </button>
        }
    });

    html! {
        <>
            <div class="section__title exc-variants__head">
                <h2 class="section__title">{ "Distinct examples" }</h2>
                <div class="exc-variants__nav">
                    <button class="btn btn--small" onclick={go(-1)} disabled={count <= 1}
                        aria-label="Previous example">
                        { icons::chevron_left() }
                    </button>
                    <span class="exc-variants__pos">{ format!("{} of {}", i + 1, count) }</span>
                    <button class="btn btn--small" onclick={go(1)} disabled={count <= 1}
                        aria-label="Next example">
                        { icons::chevron_right() }
                    </button>
                </div>
            </div>
            <div class="occurrence exc-variant">
                <div class="occurrence__meta">
                    <span class="badge badge--brand" title="Occurrences sharing this exact message and stack trace">
                        { format!("×{} occurrence{}", group_thousands(variant.count), if variant.count == 1 { "" } else { "s" }) }
                    </span>
                    <span>{ ua }</span>
                    if let Some(app) = app {
                        <span class="exc-variant__app"><code>{ app }</code></span>
                    }
                    <span class={classes!("badge", if variant.handled { "badge--muted" } else { "badge--warn" })}>
                        { if variant.handled { "handled" } else { "unhandled" } }
                    </span>
                    <span class="muted">{ format!("last seen {}", ago(variant.last_seen_ms)) }</span>
                    // Set apart from the descriptive metadata, at the row's end.
                    <span class="occurrence__actions">{ trace_link }</span>
                </div>
                <div class="occurrence__message">
                    { &variant.message }
                    { metadata.tag_pills() }
                </div>
                { metadata.context_list() }
                if let Some(stack) = &variant.stack {
                    <pre class="stack">{ stack }</pre>
                }
            </div>
        </>
    }
}
