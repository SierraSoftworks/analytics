//! One session in forensic detail: the visit's context (source, client,
//! locale, claimed release) and a vertical timeline of its page views, custom
//! events, and exceptions, in arrival order. Page views fold their unload
//! beacon in as a time-on-page badge.

use std::collections::{HashMap, HashSet};

use analytics_api::{SessionTrace as SessionTraceData, TraceEvent, TraceEventKind, source_label};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::metadata::Metadata;
use crate::components::{ApiErrorAlert, Crumb, PageHeader};
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
    let crumbs = vec![
        Crumb::link_with_query("Dashboard", Route::Overview, filters.to_pairs()),
        Crumb::current(title.clone()),
    ];

    html! {
        <div class="page">
            <PageHeader crumbs={crumbs} title={title}
                subtitle="Everything this visit reported, in order: pages, events, and exceptions." />
            {
                match &*data {
                    None => html! { <div class="page-loading">{ "Loading…" }</div> },
                    Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
                    Some(Ok(trace)) => {
                        let count = |kind: TraceEventKind| {
                            trace.events.iter().filter(|e| e.kind == kind).count() as i64
                        };
                        html! {
                            <>
                                { summary_panel(trace) }
                                <div class="trace-timeline__head">
                                    <h2 class="section__title">{ "Timeline" }</h2>
                                    <span class="muted">
                                        { trace_counts(
                                            count(TraceEventKind::PageLoad),
                                            count(TraceEventKind::Custom),
                                            count(TraceEventKind::Exception),
                                        ) }
                                    </span>
                                </div>
                                { timeline(trace) }
                            </>
                        }
                    }
                }
            }
        </div>
    }
}

/// The visit's context card: one fact per cell, absent dimensions dropped.
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
        value.map(|value| {
            html! {
                <div class="trace-fact" key={label}>
                    <span class="trace-fact__label">{ label }</span>
                    <span class="trace-fact__value" title={value.clone()}>{ value }</span>
                </div>
            }
        })
    });

    html! { <div class="panel trace-facts">{ for cells }</div> }
}

/// The vertical timeline. A page view's unload (matched by beacon id) is
/// folded into the view's row as its time-on-page; unloads whose load fell
/// outside the returned window still render on their own.
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

    let started = trace.started_ms;
    let rows = trace.events.iter().enumerate().filter_map(|(i, event)| {
        // Fold paired unloads into their page view's row.
        if event.kind == TraceEventKind::PageUnload && loads.contains(event.bid.as_str()) {
            return None;
        }
        let duration = match event.kind {
            TraceEventKind::PageLoad => durations.get(event.bid.as_str()).copied(),
            TraceEventKind::PageUnload => event.duration_ms,
            _ => None,
        };
        Some(entry(event, started, duration, i))
    });

    html! { <ol class="trace-timeline">{ for rows }</ol> }
}

fn entry(event: &TraceEvent, started_ms: i64, duration: Option<i64>, index: usize) -> Html {
    let (modifier, label) = match event.kind {
        TraceEventKind::PageLoad => ("page", "Page view"),
        TraceEventKind::PageUnload => ("page", "Page exit"),
        TraceEventKind::Custom => ("event", "Event"),
        TraceEventKind::Exception => ("exception", "Exception"),
    };
    let offset = event.received_ms - started_ms;

    let body = match event.kind {
        TraceEventKind::PageLoad | TraceEventKind::PageUnload => html! {
            <div class="trace-entry__line">
                <code class="trace-entry__path">
                    { event.pathname.clone().unwrap_or_else(|| "/".into()) }
                </code>
                if let Some(duration) = duration {
                    <span class="badge badge--muted" title="Time on page">
                        { format!("{} on page", format_duration(duration)) }
                    </span>
                }
            </div>
        },
        TraceEventKind::Custom => {
            let metadata = Metadata::parse(event.metadata.as_deref());
            html! {
                <>
                    <div class="trace-entry__line trace-entry__line--wrap">
                        <strong class="trace-entry__name">
                            { event.event_name.clone().unwrap_or_else(|| "event".into()) }
                        </strong>
                        { metadata.tag_pills() }
                    </div>
                    { metadata.context_list() }
                </>
            }
        }
        TraceEventKind::Exception => {
            let handled = event.exc_handled.unwrap_or(false);
            let metadata = Metadata::parse(event.metadata.as_deref());
            html! {
                <>
                    <div class="trace-entry__line trace-entry__line--wrap">
                        <code class="trace-entry__exc-type">
                            { event.exc_type.clone().unwrap_or_else(|| "Error".into()) }
                        </code>
                        <span class="trace-entry__exc-message">
                            { event.exc_message.clone().unwrap_or_default() }
                        </span>
                        if !handled {
                            <span class="badge badge--warn">{ "unhandled" }</span>
                        }
                        { metadata.tag_pills() }
                    </div>
                    { metadata.context_list() }
                    if let Some(stack) = &event.exc_stack {
                        <pre class="stack">{ stack.clone() }</pre>
                    }
                </>
            }
        }
    };

    html! {
        <li class={classes!("trace-entry", format!("trace-entry--{modifier}"))} key={index.to_string()}>
            <span class="trace-entry__marker" aria-hidden="true" />
            <div class="trace-entry__body">
                <div class="trace-entry__head">
                    <span class="trace-entry__kind">{ label }</span>
                    <span class="trace-entry__time" title={tooltip_label(event.received_ms, 0)}>
                        { clock_time(event.received_ms) }
                    </span>
                    <span class="trace-entry__offset" title="Since the session started">
                        { format!("+{}", format_duration(offset)) }
                    </span>
                </div>
                { body }
            </div>
        </li>
    }
}
