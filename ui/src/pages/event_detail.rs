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
use yewi::prelude::{
    Button, Heading, HeadingLevel, InlineCode, NavTarget, PageHeader, Pill, Size, Text, TextColor,
    TextSize, TextTag, Tone,
};

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::metadata::Metadata;
use crate::components::page::{crumbs, loading};
use crate::components::{ApiErrorAlert, Scrubber, Sparkline, TraceList, distribution};
use crate::filters::{query_param, use_filters};
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
    let breadcrumbs = crumbs("Dashboard", Route::Overview, &filters, &title);

    let body = if name.is_empty() {
        html! {
            <ApiErrorAlert error={ApiError::Server(
                "No event was specified. Open this page from the dashboard's Events panel.".into()
            )} />
        }
    } else {
        match &*detail {
            None => loading(),
            Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
            Some(Ok(detail)) => {
                let summary = format!(
                    "{} occurrences · first seen {} · last seen {}",
                    group_thousands(detail.count),
                    ago(detail.first_seen_ms),
                    ago(detail.last_seen_ms),
                );
                // Accent-coloured bars: event volume is traffic, not trouble.
                let tone = Tone::Neutral;
                html! {
                    <>
                        <section class="detail-head">
                            <Text tag={TextTag::P} color={TextColor::Muted}>{ summary }</Text>
                            if !detail.trend.is_empty() {
                                <div class="detail-head__trend">
                                    <Text size={TextSize::Xs} color={TextColor::Subtle} class="eyebrow">
                                        { range_label.clone() }
                                    </Text>
                                    <Sparkline points={detail.trend.clone()} />
                                </div>
                            }
                        </section>

                        <section class="section">
                            <header class="section__head">
                                <Heading level={HeadingLevel::H2} size={TextSize::Md}>{ "Distribution" }</Heading>
                            </header>
                            <div class="dist-grid">
                                { distribution("Apps / sources", &sources_named(&detail.breakdowns.sources), detail.count, tone) }
                                { distribution("Pages", &detail.breakdowns.pages, detail.count, tone) }
                                { distribution("Applications", &detail.breakdowns.browsers, detail.count, tone) }
                                { distribution("Operating systems", &detail.breakdowns.operating_systems, detail.count, tone) }
                                { distribution("Devices", &detail.breakdowns.devices, detail.count, tone) }
                                { distribution("Countries", &countries_named(&detail.breakdowns.countries), detail.count, tone) }
                                { distribution("Languages", &languages_named(&detail.breakdowns.languages), detail.count, tone) }
                            </div>
                        </section>

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
            <PageHeader {title} {breadcrumbs} meta={html! { <Pill dot=false tone={Tone::Info}>{ "Event" }</Pill> }} />
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

/// Scrub through the event's distinct examples; each shows
/// how many occurrences share its exact metadata.
#[function_component(VariantScrubber)]
fn variant_scrubber(props: &VariantScrubberProps) -> Html {
    let index = use_state(|| 0usize);
    let filters = use_filters();
    let count = props.variants.len();
    if count == 0 {
        return html! {};
    }
    let i = (*index).min(count - 1);
    let variant = &props.variants[i];
    let on_change = {
        let index = index.clone();
        Callback::from(move |next: usize| index.set(next))
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
        let href = NavTarget::route(Route::Trace { id: sid.clone() }).query(filters.to_pairs());
        html! {
            <Button size={Size::Small} {href}
                attrs={yewi::attrs! { "title" => format!("Open session {}", short_session_id(sid)) }}>
                { "View session trace" }
            </Button>
        }
    });

    html! {
        <Scrubber {count} index={i} {on_change}>
            <div class="example__meta">
                <Pill dot=false tone={Tone::Info}
                    attrs={yewi::attrs! { "title" => "Occurrences sharing this exact metadata" }}>
                    { format!("×{} occurrence{}", group_thousands(variant.count), if variant.count == 1 { "" } else { "s" }) }
                </Pill>
                <span>{ ua }</span>
                if let Some(fired_at) = fired_at {
                    <InlineCode>{ fired_at }</InlineCode>
                }
                <Text color={TextColor::Subtle}>{ format!("last seen {}", ago(variant.last_seen_ms)) }</Text>
                // Set apart from the descriptive metadata, at the row's end.
                <span class="example__actions">{ trace_link }</span>
            </div>
            if !metadata.tags.is_empty() {
                <div class="example__tags">{ metadata.tag_pills() }</div>
            }
            { metadata.context_list() }
            if metadata.is_empty() {
                <Text tag={TextTag::P} color={TextColor::Subtle}>{ "No metadata reported." }</Text>
            }
        </Scrubber>
    }
}
