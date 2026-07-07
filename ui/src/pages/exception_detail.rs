//! One exception group in forensic detail: triage controls, the occurrence
//! trend, how failures distribute across key dimensions (app version, OS,
//! browser, …), and a scrubber over the group's **distinct variants** — one
//! representative example per unique message/stack, with a count of the
//! occurrences it stands for.

use analytics_api::{
    CountRow, ExceptionGroupDetail, ExceptionStatus, ExceptionVariant, TriageInput,
};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::status::{status_class, status_label};
use crate::components::{ApiErrorAlert, Crumb, PageHeader, Sparkline, icons};
use crate::filters::{use_filters, use_navigate_with_filters};
use crate::format::{ago, compact, group_thousands, short_session_id};

#[derive(Properties, PartialEq)]
pub struct ExceptionDetailProps {
    pub project: String,
    pub group: String,
}

#[function_component(ExceptionDetail)]
pub fn exception_detail(props: &ExceptionDetailProps) -> Html {
    let (project, group) = (props.project.clone(), props.group.clone());
    let detail = use_state(|| None::<Result<ExceptionGroupDetail, ApiError>>);
    let reload = use_state(|| 0u32);
    // The row that linked here carried the inbox's filter state, so the detail
    // numbers cover the same window as the row the operator just read.
    let filters = use_filters();
    let range_label = filters.range_label();

    {
        let detail = detail.clone();
        let (project, group) = (project.clone(), group.clone());
        let filters = filters.clone();
        // `project` is a dependency: two projects can share a fingerprint, so
        // navigating between same-group exceptions across projects must refetch.
        use_effect_with(
            (project.clone(), group.clone(), filters.canonical(), *reload),
            move |_| {
                let range = filters.range_query(js_sys::Date::now() as i64);
                spawn_local(async move {
                    detail.set(Some(api::exception_detail(&group, &project, &range).await));
                });
                || ()
            },
        );
    }

    let set_status = {
        let (project, group, reload) = (project.clone(), group.clone(), reload.clone());
        move |status: ExceptionStatus| {
            let input = TriageInput {
                project_id: project.clone(),
                status,
                note: None,
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
                    Some(Ok(detail)) => html! {
                        <>
                            <div class="exc-head panel">
                                <div class="exc-head__status">
                                    <span class={status_class(detail.group.status)}>{ status_label(detail.group.status) }</span>
                                    <p class="exc-head__message">{ &detail.group.sample_message }</p>
                                    <div class="exc-head__meta muted">
                                        { format!(
                                            "{} occurrences · first seen {} · last seen {}",
                                            group_thousands(detail.group.count),
                                            ago(detail.group.first_seen_ms),
                                            ago(detail.group.last_seen_ms),
                                        ) }
                                    </div>
                                    <div class="exc-head__actions">
                                        <button class="btn" onclick={set_status(ExceptionStatus::Resolved)}>{ "Mark resolved" }</button>
                                        <button class="btn" onclick={set_status(ExceptionStatus::Ignored)}>{ "Ignore" }</button>
                                        <button class="btn btn--ghost" onclick={set_status(ExceptionStatus::Unresolved)}>{ "Reopen" }</button>
                                    </div>
                                </div>
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
                                { distribution("Apps / sources", &detail.breakdowns.sources, detail.group.count) }
                                { distribution("Browsers", &detail.breakdowns.browsers, detail.group.count) }
                                { distribution("Operating systems", &detail.breakdowns.operating_systems, detail.group.count) }
                                { distribution("Devices", &detail.breakdowns.devices, detail.group.count) }
                            </div>

                            <VariantScrubber
                                variants={detail.variants.clone()}
                                key={detail.group.group_id.clone()}
                            />
                        </>
                    },
                }
            }
        </div>
    }
}

/// One distribution card: proportional bars over a dimension's values. Cards
/// whose only row is the absent sentinel carry no signal and are dropped.
fn distribution(title: &str, rows: &[CountRow], total: i64) -> Html {
    let informative = rows.iter().any(|r| !r.key.is_empty());
    if rows.is_empty() || !informative {
        return html! {};
    }
    let max = rows.iter().map(|r| r.count).max().unwrap_or(1).max(1);
    let total = total.max(1);
    let bars = rows.iter().map(|row| {
        let share = row.count as f64 / total as f64 * 100.0;
        let width = row.count as f64 / max as f64 * 100.0;
        let label = if row.key.is_empty() { "Unknown".to_string() } else { row.key.clone() };
        html! {
            <li class="dist__row" key={row.key.clone()}>
                <span class="dist__bar" style={format!("width: {width:.1}%")} />
                <span class={classes!("dist__label", row.key.is_empty().then_some("brow__text--absent"))}
                    title={label.clone()}>
                    { label }
                </span>
                <span class="dist__share">{ format!("{share:.0}%") }</span>
                <span class="dist__count">{ compact(row.count) }</span>
            </li>
        }
    });
    html! {
        <section class="dist">
            <h3 class="dist__title">{ title.to_string() }</h3>
            <ul class="dist__rows">{ for bars }</ul>
        </section>
    }
}

#[derive(Properties, PartialEq)]
struct VariantScrubberProps {
    variants: Vec<ExceptionVariant>,
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
    // The source is the application; pair it with the reported release.
    let app = variant.source.as_ref().map(|source| {
        let label = analytics_api::source_label(source).to_string();
        match &variant.app_version {
            Some(version) => format!("{label} @ {version}"),
            None => label,
        }
    });

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
                    { trace_link }
                </div>
                <div class="occurrence__message">{ &variant.message }</div>
                { metadata_table(variant.metadata.as_deref()) }
                if let Some(stack) = &variant.stack {
                    <pre class="stack">{ stack }</pre>
                }
            </div>
        </>
    }
}

/// The reporter-supplied metadata of the variant's latest occurrence, rendered
/// as a key/value grid (falling back to the raw JSON if it isn't an object).
fn metadata_table(metadata: Option<&str>) -> Html {
    let Some(raw) = metadata.filter(|m| !m.trim().is_empty()) else {
        return html! {};
    };
    match serde_json::from_str::<std::collections::BTreeMap<String, serde_json::Value>>(raw) {
        Ok(fields) if !fields.is_empty() => {
            let rows = fields.iter().map(|(key, value)| {
                let value = match value {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                html! {
                    <div class="exc-meta__row" key={key.clone()}>
                        <span class="exc-meta__key">{ key.clone() }</span>
                        <span class="exc-meta__value" title={value.clone()}>{ value.clone() }</span>
                    </div>
                }
            });
            html! {
                <div class="exc-meta">
                    <span class="exc-meta__title">{ "Metadata" }</span>
                    <div class="exc-meta__rows">{ for rows }</div>
                </div>
            }
        }
        _ => html! {
            <div class="exc-meta">
                <span class="exc-meta__title">{ "Metadata" }</span>
                <pre class="stack">{ raw.to_string() }</pre>
            </div>
        },
    }
}
