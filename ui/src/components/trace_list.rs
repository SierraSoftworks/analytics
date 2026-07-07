//! The session-trace sample list: a vertical timeline of recent sessions, each
//! row identifying the visit (where it landed, on what client, from where) and
//! opening the full trace view. Shared by the dashboard ("recent sessions
//! matching the filters") and the exception detail page ("sessions this group
//! occurred in").

use analytics_api::{TraceSummary, source_label};
use yew::prelude::*;

use crate::app::Route;
use crate::components::icons;
use crate::filters::{use_filters, use_navigate_with_filters};
use crate::format::{
    ago, country_flag, country_name, short_session_id, tooltip_label, trace_counts,
};

#[derive(Properties, PartialEq)]
pub struct TraceListProps {
    pub traces: Vec<TraceSummary>,
    #[prop_or(AttrValue::Static("Session traces"))]
    pub title: AttrValue,
    #[prop_or_default]
    pub hint: Option<AttrValue>,
    /// Shown when the list is empty; pass `None` to render nothing instead.
    #[prop_or_default]
    pub empty: Option<AttrValue>,
}

#[function_component(TraceList)]
pub fn trace_list(props: &TraceListProps) -> Html {
    let filters = use_filters();
    let navigate = use_navigate_with_filters();

    if props.traces.is_empty() && props.empty.is_none() {
        return html! {};
    }

    let entries = props.traces.iter().map(|trace| {
        let open = {
            let (navigate, filters) = (navigate.clone(), filters.clone());
            let id = trace.session_id.clone();
            move || navigate.emit((Route::Trace { id: id.clone() }, filters.clone()))
        };
        let onclick = {
            let open = open.clone();
            Callback::from(move |_: MouseEvent| open())
        };
        let onkeydown = Callback::from(move |e: KeyboardEvent| {
            if matches!(e.key().as_str(), "Enter" | " ") {
                e.prevent_default();
                open();
            }
        });

        // The visit reads as one location: source followed by the entry page.
        let location = format!(
            "{}{}",
            source_label(&trace.source),
            trace.entry_path.as_deref().unwrap_or("")
        );
        // The client pill: application (browser or app) + its version, marked
        // with a matching icon; app runs additionally carry the reported release.
        let is_app = trace.ua_device.as_deref() == Some("App");
        let client = trace.ua_browser.as_ref().map(|browser| {
            match &trace.ua_version {
                Some(version) => format!("{browser} {version}"),
                None => browser.clone(),
            }
        });
        let country = trace.country.as_ref().map(|code| match country_flag(code) {
            Some(flag) => format!("{flag} {}", country_name(code)),
            None => country_name(code),
        });

        html! {
            <li class="trace-row" key={trace.session_id.clone()} role="button" tabindex="0"
                {onclick} {onkeydown}>
                <span class={classes!("trace-row__marker", (trace.exceptions > 0).then_some("trace-row__marker--exceptions"))}
                    aria-hidden="true" />
                <div class="trace-row__body">
                    <div class="trace-row__head">
                        <code class="trace-row__id">{ short_session_id(&trace.session_id) }</code>
                        <span class="trace-row__time" title={tooltip_label(trace.started_ms, 0)}>
                            { ago(trace.started_ms) }
                        </span>
                        <span class="trace-row__counts muted">
                            { trace_counts(trace.pageviews, trace.events, 0) }
                            // Exceptions are the attention signal: set apart in
                            // the status colour rather than folded into the
                            // neutral counts.
                            if trace.exceptions > 0 {
                                <span class="trace-row__exceptions">
                                    { format!(" · {} exception{}", trace.exceptions,
                                        if trace.exceptions == 1 { "" } else { "s" }) }
                                </span>
                            }
                        </span>
                        <span class="trace-row__open" aria-hidden="true">{ icons::chevron_right() }</span>
                    </div>
                    <div class="trace-row__meta">
                        <code class="trace-row__location" title={location.clone()}>{ location }</code>
                        if let Some(client) = client {
                            <span class="trace-row__client"
                                title={if is_app { "Application" } else { "Browser" }}>
                                <span class="trace-row__client-icon" aria-hidden="true">
                                    { if is_app { icons::terminal() } else { icons::globe() } }
                                </span>
                                <span class="meta-tag meta-tag--client">
                                    { client }
                                    if let Some(version) = &trace.app_version {
                                        <span class="trace-row__release">{ format!("v{version}") }</span>
                                    }
                                </span>
                            </span>
                        }
                        if let Some(country) = country {
                            <span>{ country }</span>
                        }
                    </div>
                </div>
            </li>
        }
    });

    html! {
        <div class="panel trace-sample">
            <div class="panel__head">
                <h2 class="panel__title">{ props.title.clone() }</h2>
                if let Some(hint) = &props.hint {
                    <span class="panel__hint">{ hint.clone() }</span>
                }
            </div>
            if props.traces.is_empty() {
                if let Some(empty) = &props.empty {
                    <p class="trace-sample__empty">{ empty.clone() }</p>
                }
            } else {
                <ol class="trace-sample__list">{ for entries }</ol>
            }
        </div>
    }
}
