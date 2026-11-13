//! One session in forensic detail: the visit's context (source, client,
//! locale, claimed release) and a vertical timeline of its page views. Each
//! page view spans from its load to its unload (a time-on-page badge), and the
//! custom events and exceptions that fired while it was open — matched by
//! beacon id — hang beneath it on a nested child timeline.

use std::collections::{HashMap, HashSet};

use analytics_api::{SessionTrace as SessionTraceData, TraceEvent, TraceEventKind, source_label};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yewi::prelude::{
    CodeBlock, DescriptionItem, DescriptionLayout, DescriptionList, Heading, HeadingLevel,
    InlineCode, PageHeader, Pill, Text, TextColor, TextSize, Timeline, TimelineItem,
    TimelineRunOff, Tone, attrs, icons,
};

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::ApiErrorAlert;
use crate::components::metadata::Metadata;
use crate::components::page::{crumbs, loading};
use crate::filters::use_filters;
use crate::format::{
    clock_time, country_flag, country_name, format_duration, language_name, short_session_id,
    tooltip_label, trace_counts,
};

#[derive(Properties, PartialEq)]
pub struct TraceProps {
    pub id: String,
}

#[function_component(Trace)]
pub fn trace(props: &TraceProps) -> Html {
    let data = use_state(|| None::<Result<SessionTraceData, ApiError>>);
    {
        let data = data.clone();
        use_effect_with(props.id.clone(), move |id| {
            let id = id.clone();
            spawn_local(async move {
                data.set(Some(api::session_trace(&id).await));
            });
            || ()
        });
    }

    // The entry that linked here carried the active filter state; the crumb
    // hands it back so the dashboard reopens the way the operator left it.
    let filters = use_filters();
    let title = format!("Session {}", short_session_id(&props.id));
    let breadcrumbs = crumbs("Dashboard", Route::Overview, &filters, &title);

    html! {
        <div class="page">
            <PageHeader {title} {breadcrumbs}
                subtitle="Everything this visit reported, in order: pages, events, and exceptions." />
            {
                match &*data {
                    None => loading(),
                    Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
                    Some(Ok(trace)) => {
                        let count = |kind: TraceEventKind| {
                            trace.events.iter().filter(|e| e.kind == kind).count() as i64
                        };
                        html! {
                            <>
                                { summary_panel(trace) }
                                <section class="section">
                                    <header class="section__head">
                                        <Heading level={HeadingLevel::H2} size={TextSize::Md}>{ "Timeline" }</Heading>
                                        <Text size={TextSize::Sm} color={TextColor::Subtle}>
                                            { trace_counts(
                                                count(TraceEventKind::PageLoad),
                                                count(TraceEventKind::Custom),
                                                count(TraceEventKind::Exception),
                                            ) }
                                        </Text>
                                    </header>
                                    { timeline(trace) }
                                </section>
                            </>
                        }
                    }
                }
            }
        </div>
    }
}

/// The visit's context: one fact per cell, absent dimensions dropped.
fn summary_panel(trace: &SessionTraceData) -> Html {
    let client = match (&trace.ua_browser, &trace.ua_version) {
        (Some(browser), Some(version)) => Some(format!("{browser} {version}")),
        (Some(browser), None) => Some(browser.clone()),
        _ => None,
    };
    let country = trace.country.as_ref().map(|code| match country_flag(code) {
        Some(flag) => format!("{flag} {}", country_name(code)),
        None => country_name(code),
    });
    let span = trace.ended_ms - trace.started_ms;

    let facts = [
        ("Source", Some(source_label(&trace.source).to_string())),
        ("Started", Some(tooltip_label(trace.started_ms, 0))),
        ("Duration", (span > 0).then(|| format_duration(span))),
        ("Client", client),
        ("OS", trace.ua_os.clone()),
        ("Country", country),
        ("Language", trace.language.as_deref().map(language_name)),
        ("App version", trace.app_version.clone()),
    ];

    let cells = facts.into_iter().filter_map(|(label, value)| {
        value.map(
            |value| html! { <DescriptionItem key={label} term={label}>{ value }</DescriptionItem> },
        )
    });

    html! {
        <DescriptionList layout={DescriptionLayout::Stacked} min="10rem" class="trace-facts">
            { for cells }
        </DescriptionList>
    }
}

/// The vertical timeline. Page views form the parent spine: each spans from its
/// load to its unload (a time-on-page badge), matched by beacon id. The custom
/// events and exceptions that share a page view's beacon id hang beneath it on a
/// nested child timeline — so it reads at a glance which activity happened
/// within which page view. Events whose page view fell outside the returned
/// window (and unloads whose load did) still render on the parent spine.
fn timeline(trace: &SessionTraceData) -> Html {
    let mut durations: HashMap<&str, i64> = HashMap::new();
    let mut loads: HashSet<&str> = HashSet::new();
    for event in &trace.events {
        match event.kind {
            TraceEventKind::PageLoad if !event.bid.is_empty() => {
                loads.insert(event.bid.as_str());
            }
            TraceEventKind::PageUnload if !event.bid.is_empty() => {
                if let Some(duration) = event.duration_ms {
                    durations.insert(event.bid.as_str(), duration);
                }
            }
            _ => {}
        }
    }

    // Bucket each page view's events/exceptions under its beacon id, preserving
    // arrival order. Only events whose page view is present in this window are
    // nested; the rest fall through to the parent spine as orphans.
    let mut children: HashMap<&str, Vec<(usize, &TraceEvent)>> = HashMap::new();
    for (i, event) in trace.events.iter().enumerate() {
        if matches!(
            event.kind,
            TraceEventKind::Custom | TraceEventKind::Exception
        ) && loads.contains(event.bid.as_str())
        {
            children
                .entry(event.bid.as_str())
                .or_default()
                .push((i, event));
        }
    }

    let started = trace.started_ms;
    let rows = trace.events.iter().enumerate().filter_map(|(i, event)| {
        let paired = !event.bid.is_empty() && loads.contains(event.bid.as_str());
        match event.kind {
            TraceEventKind::PageLoad => {
                let duration = durations.get(event.bid.as_str()).copied();
                let kids = children.get(event.bid.as_str());
                Some(page_view(event, started, duration, kids, i))
            }
            // Paired unloads and nested events are drawn within their page view.
            TraceEventKind::PageUnload if paired => None,
            TraceEventKind::Custom | TraceEventKind::Exception if paired => None,
            // Orphans (page view outside the window) still get a parent row.
            _ => Some(entry(event, started, event.duration_ms, i)),
        }
    });

    html! { <Timeline label="Session timeline" run_off={TimelineRunOff::None}>{ for rows }</Timeline> }
}

/// When an entry happened: the wall-clock time, and how far into the session.
fn when(event: &TraceEvent, started_ms: i64) -> Html {
    html! {
        <span class="trace-when">
            <span title={tooltip_label(event.received_ms, 0)}>{ clock_time(event.received_ms) }</span>
            <span title="Since the session started">
                { format!("+{}", format_duration(event.received_ms - started_ms)) }
            </span>
        </span>
    }
}

/// A page's path, with how long it stayed open when its unload was seen.
fn page_line(event: &TraceEvent, duration: Option<i64>) -> (Html, Html) {
    let title = html! {
        <InlineCode>{ event.pathname.clone().unwrap_or_else(|| "/".into()) }</InlineCode>
    };
    let aside = duration
        .map(|duration| {
            html! {
                <Pill dot=false attrs={attrs! { "title" => "Time on page" }}>
                    { format!("{} on page", format_duration(duration)) }
                </Pill>
            }
        })
        .unwrap_or_default();
    (title, aside)
}

/// A page view on the parent spine: its path and time-on-page span, with the
/// events and exceptions that fired while it was open nested beneath it. The
/// nested timeline is omitted entirely when the page view saw no such activity.
fn page_view(
    event: &TraceEvent,
    started_ms: i64,
    duration: Option<i64>,
    children: Option<&Vec<(usize, &TraceEvent)>>,
    index: usize,
) -> Html {
    let nested = children.filter(|kids| !kids.is_empty()).map(|kids| {
        let rows = kids.iter().map(|(i, ev)| entry(ev, started_ms, None, *i));
        html! {
            <Timeline label="During this page view" run_off={TimelineRunOff::None}>{ for rows }</Timeline>
        }
    });
    let (title, aside) = page_line(event, duration);

    html! {
        <TimelineItem key={index.to_string()} icon={icons::FILE_TEXT} {title} {aside}
            meta={html! { <>{ "Page view · " }{ when(event, started_ms) }</> }}>
            { nested.unwrap_or_default() }
        </TimelineItem>
    }
}

fn entry(event: &TraceEvent, started_ms: i64, duration: Option<i64>, index: usize) -> Html {
    let key = index.to_string();
    match event.kind {
        TraceEventKind::PageLoad | TraceEventKind::PageUnload => {
            let label = if event.kind == TraceEventKind::PageLoad {
                "Page view · "
            } else {
                "Page exit · "
            };
            let (title, aside) = page_line(event, duration);
            html! {
                <TimelineItem {key} icon={icons::FILE_TEXT} {title} {aside}
                    meta={html! { <>{ label }{ when(event, started_ms) }</> }} />
            }
        }
        TraceEventKind::Custom => {
            let metadata = Metadata::parse(event.metadata.as_deref());
            let title = html! { { event.event_name.clone().unwrap_or_else(|| "event".into()) } };
            html! {
                <TimelineItem {key} icon={icons::ZAP} tone={Tone::Info} {title}
                    meta={html! { <>{ "Event · " }{ when(event, started_ms) }</> }}>
                    if !metadata.tags.is_empty() {
                        <div class="example__tags">{ metadata.tag_pills() }</div>
                    }
                    { metadata.context_list() }
                </TimelineItem>
            }
        }
        TraceEventKind::Exception => {
            let handled = event.exc_handled.unwrap_or(false);
            let metadata = Metadata::parse(event.metadata.as_deref());
            let title = html! {
                <>
                    <InlineCode>{ event.exc_type.clone().unwrap_or_else(|| "Error".into()) }</InlineCode>
                    { " " }
                    { event.exc_message.clone().unwrap_or_default() }
                </>
            };
            let aside = if handled {
                Html::default()
            } else {
                html! { <Pill dot=false tone={Tone::Warn}>{ "unhandled" }</Pill> }
            };
            html! {
                <TimelineItem {key} icon={icons::BUG} tone={Tone::Danger} tone_label="exception" {title} {aside}
                    meta={html! { <>{ "Exception · " }{ when(event, started_ms) }</> }}>
                    if !metadata.tags.is_empty() {
                        <div class="example__tags">{ metadata.tag_pills() }</div>
                    }
                    { metadata.context_list() }
                    if let Some(stack) = &event.exc_stack {
                        <CodeBlock code={stack.clone()} wrap=true copy_label="Copy stack trace" />
                    }
                </TimelineItem>
            }
        }
    }
}
