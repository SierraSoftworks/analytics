//! The session-trace sample list: a vertical timeline of recent sessions, each
//! entry identifying the visit (where it landed, on what client, from where) and
//! opening the full trace view. Shared by the dashboard ("recent sessions
//! matching the filters") and the exception detail page ("sessions this group
//! occurred in").

use analytics_api::{TraceSummary, source_label};
use yew::prelude::*;
use yewi::prelude::{
    Heading, HeadingLevel, Icon, NavTarget, Tag, Text, TextColor, TextSize, TextTag, Timeline,
    TimelineItem, TimelineRunOff, Tone, icons,
};

use crate::app::Route;
use crate::filters::use_filters;
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

    if props.traces.is_empty() && props.empty.is_none() {
        return html! {};
    }

    let entries = props.traces.iter().map(|trace| {
        // The entry that links to a trace carries the filter state, so the
        // trace's breadcrumb can hand it back.
        let href = NavTarget::route(Route::Trace { id: trace.session_id.clone() }).query(filters.to_pairs());

        // The visit reads as one location: source followed by the entry page.
        let location = format!(
            "{}{}",
            source_label(&trace.source),
            trace.entry_path.as_deref().unwrap_or("")
        );
        // The client tag: application (browser or app) + its version, marked
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

        let title = html! {
            <>
                <code class="trace-row__id">{ short_session_id(&trace.session_id) }</code>
                <span class="trace-row__location" title={location.clone()}>{ location }</span>
            </>
        };
        let meta = html! {
            <span class="trace-row__meta">
                <span>{ trace_counts(trace.pageviews, trace.events, 0) }</span>
                // Exceptions are the attention signal: set apart in the
                // status colour rather than folded into the neutral counts.
                if trace.exceptions > 0 {
                    <Text tone={Tone::Danger} weight={yewi::TextWeight::Medium}>
                        { format!("{} exception{}", trace.exceptions,
                            if trace.exceptions == 1 { "" } else { "s" }) }
                    </Text>
                }
                if let Some(client) = client {
                    <Tag icon={if is_app { icons::TERMINAL } else { icons::GLOBE }} mono=true
                        attrs={yewi::attrs! { "title" => if is_app { "Application" } else { "Browser" } }}>
                        { client }
                        if let Some(version) = &trace.app_version {
                            { format!(" · v{version}") }
                        }
                    </Tag>
                }
                if let Some(country) = country {
                    <span>{ country }</span>
                }
            </span>
        };
        let aside = html! {
            <span class="trace-row__aside">
                <Text size={TextSize::Sm} color={TextColor::Subtle}
                    attrs={yewi::attrs! { "title" => tooltip_label(trace.started_ms, 0) }}>
                    { ago(trace.started_ms) }
                </Text>
                <Icon icon={icons::CHEVRON_RIGHT} />
            </span>
        };

        html! {
            <TimelineItem key={trace.session_id.clone()} {title} {meta} {aside} {href}
                tone={(trace.exceptions > 0).then_some(Tone::Danger)}
                tone_label={(trace.exceptions > 0).then_some("has exceptions")} />
        }
    });

    html! {
        <section class="section trace-sample">
            <header class="section__head">
                <Heading level={HeadingLevel::H2} size={TextSize::Md}>{ props.title.clone() }</Heading>
                if let Some(hint) = &props.hint {
                    <Text size={TextSize::Sm} color={TextColor::Subtle}>{ hint.clone() }</Text>
                }
            </header>
            if props.traces.is_empty() {
                if let Some(empty) = &props.empty {
                    <Text tag={TextTag::P} color={TextColor::Subtle}>{ empty.clone() }</Text>
                }
            } else {
                <Timeline run_off={TimelineRunOff::None} divided=true label={props.title.clone()}>
                    { for entries }
                </Timeline>
            }
        </section>
    }
}
