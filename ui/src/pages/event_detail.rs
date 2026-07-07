//! One custom/pixel event in forensic detail, modelled on the exception view:
//! the occurrence trend, how the event distributes across key dimensions
//! (source, page, application, …), a scrubber over its **distinct metadata
//! variants** — one representative example per unique reporter payload — and
//! the session traces it occurred in.
//!
//! The event name rides in the query string (`?name=…`) alongside the filter
//! state, because names are reporter-chosen free text that must not break the
//! router; the filters scope the numbers to the same slice as the dashboard
//! panel that linked here.

use analytics_api::{CountRow, EventDetail as EventDetailData, EventVariant, source_label};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::use_location;

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::metadata::Metadata;
use crate::components::{
    ApiErrorAlert, Crumb, PageHeader, Sparkline, TraceList, distribution, icons,
};
use crate::filters::{query_param, use_filters, use_navigate_with_filters};
use crate::format::{
    ago, country_flag, country_name, group_thousands, language_name, short_session_id,
};

#[function_component(EventDetail)]
pub fn event_detail() -> Html {
    let location = use_location();
    let name = location
        .as_ref()
        .and_then(|l| query_param(l.query_str(), "name"))
        .unwrap_or_default();
    let detail = use_state(|| None::<Result<EventDetailData, ApiError>>);
    // The panel row that linked here carried the dashboard's filter state, so
    // the detail numbers cover the same slice the operator was looking at.
    let filters = use_filters();
    let range_label = filters.range_label();

    {
        let detail = detail.clone();
        let name = name.clone();
        let filters = filters.clone();
        use_effect_with((name.clone(), filters.canonical()), move |_| {
            if !name.is_empty() {
                let query = filters.stats_query(js_sys::Date::now() as i64);
                spawn_local(async move {
                    detail.set(Some(api::event_detail(&name, &query).await));
                });
            }
            || ()
        });
    }

    let title = if name.is_empty() {
        "Event".to_string()
    } else {
        name.clone()
    };
    let crumbs = vec![
        Crumb::link_with_query("Dashboard", Route::Overview, filters.to_pairs()),
        Crumb::current(title.clone()),
    ];

    let body = if name.is_empty() {
        html! {
            <ApiErrorAlert error={ApiError::Server(
                "No event was specified — open this page from the dashboard's Events panel.".into()
            )} />
        }
    } else {
        match &*detail {
            None => html! { <div class="page-loading">{ "Loading…" }</div> },
            Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
            Some(Ok(detail)) => {
                let meta = format!(
                    "{} occurrences · first seen {} · last seen {}",
                    group_thousands(detail.count),
                    ago(detail.first_seen_ms),
                    ago(detail.last_seen_ms),
                );
                html! {
                    <>
                        <div class="exc-head panel">
                            <div class="exc-head__top">
                                <span class="badge badge--brand">{ "Event" }</span>
                                <span class="exc-head__title">
                                    <b class="exc-head__type">{ &detail.name }</b>
                                </span>
                            </div>
                            <div class="exc-head__meta muted">{ meta }</div>
                            if !detail.trend.is_empty() {
                                <div class="exc-head__trend">
                                    <span class="stat__label">{ range_label.clone() }</span>
                                    <Sparkline points={detail.trend.clone()} class={classes!("exc-head__spark", "exc-head__spark--brand")} />
                                </div>
                            }
                        </div>

                        <h2 class="section__title">{ "Distribution" }</h2>
                        // Brand-tinted bars: event volume is traffic, not trouble.
                        <div class="dist-grid dist-grid--brand">
                            { distribution("Apps / sources", &sources_named(&detail.breakdowns.sources), detail.count) }
                            { distribution("Pages", &detail.breakdowns.pages, detail.count) }
                            { distribution("Applications", &detail.breakdowns.browsers, detail.count) }
                            { distribution("Operating systems", &detail.breakdowns.operating_systems, detail.count) }
                            { distribution("Devices", &detail.breakdowns.devices, detail.count) }
                            { distribution("Countries", &countries_named(&detail.breakdowns.countries), detail.count) }
                            { distribution("Languages", &languages_named(&detail.breakdowns.languages), detail.count) }
                        </div>

                        <VariantScrubber variants={detail.variants.clone()} key={detail.name.clone()} />
                        // Pick which of the event's sessions to inspect.
                        <TraceList
                            traces={detail.traces.clone()}
                            hint="Sessions this event occurred in"
                        />
                    </>
                }
            }
        }
    };

    html! {
        <div class="page">
            <PageHeader crumbs={crumbs} title={title} />
            { body }
        </div>
    }
}

/// Source rows re-keyed by their display label (URIs are read as bare names
/// throughout the UI).
fn sources_named(rows: &[CountRow]) -> Vec<CountRow> {
    rows.iter()
        .map(|r| CountRow {
            key: if r.key.is_empty() {
                String::new()
            } else {
                source_label(&r.key).to_string()
            },
            count: r.count,
        })
        .collect()
}

/// Country rows re-keyed as `flag name` (codes are for machines).
fn countries_named(rows: &[CountRow]) -> Vec<CountRow> {
    rows.iter()
        .map(|r| CountRow {
            key: if r.key.is_empty() {
                String::new()
            } else {
                match country_flag(&r.key) {
                    Some(flag) => format!("{flag} {}", country_name(&r.key)),
                    None => country_name(&r.key),
                }
            },
            count: r.count,
        })
        .collect()
}

/// Language rows re-keyed by their display name.
fn languages_named(rows: &[CountRow]) -> Vec<CountRow> {
    rows.iter()
        .map(|r| CountRow {
            key: if r.key.is_empty() {
                String::new()
            } else {
                language_name(&r.key)
            },
            count: r.count,
        })
        .collect()
}

#[derive(Properties, PartialEq)]
struct VariantScrubberProps {
    variants: Vec<EventVariant>,
}

/// Scrub through the event's distinct examples with ‹ › navigation; each shows
/// how many occurrences share its exact metadata.
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
    // Where the exemplar fired: source plus the page, when reported.
    let fired_at = variant.source.as_ref().map(|source| {
        format!(
            "{}{}",
            source_label(source),
            variant.pathname.as_deref().unwrap_or("")
        )
    });
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
                    <span class="badge badge--brand" title="Occurrences sharing this exact metadata">
                        { format!("×{} occurrence{}", group_thousands(variant.count), if variant.count == 1 { "" } else { "s" }) }
                    </span>
                    <span>{ ua }</span>
                    if let Some(fired_at) = fired_at {
                        <span class="exc-variant__app"><code>{ fired_at }</code></span>
                    }
                    <span class="muted">{ format!("last seen {}", ago(variant.last_seen_ms)) }</span>
                    // Set apart from the descriptive metadata, at the row's end.
                    <span class="occurrence__actions">{ trace_link }</span>
                </div>
                if !metadata.tags.is_empty() {
                    <div class="occurrence__message">
                        { metadata.tag_pills() }
                    </div>
                }
                { metadata.context_list() }
                if metadata.tags.is_empty() && metadata.context.is_empty() && metadata.raw.is_none() {
                    <div class="occurrence__message muted">{ "No metadata reported." }</div>
                }
            </div>
        </>
    }
}
