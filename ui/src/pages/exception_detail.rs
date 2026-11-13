//! One exception group in forensic detail: triage controls, the occurrence
//! trend, how failures distribute across key dimensions (app version, OS,
//! application, …), and a scrubber over the group's **distinct variants** — one
//! representative example per unique message/stack, with a count of the
//! occurrences it stands for.
//!
//! A group's identity is source-scoped — fingerprint + the application it was
//! seen on — carried as a `?source=` query parameter alongside the filter
//! state.

use analytics_api::{ExceptionGroupDetail, ExceptionVariant, TriageInput, source_label};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::use_location;
use yewi::prelude::{
    Button, ButtonGroup, CodeBlock, Heading, HeadingLevel, InlineCode, NavTarget, PageHeader, Pill,
    Size, Text, TextColor, TextSize, TextTag, Tone, icons,
};

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::metadata::Metadata;
use crate::components::page::{crumbs, loading};
use crate::components::status::status_pill;
use crate::components::{ApiErrorAlert, Scrubber, Sparkline, TraceList, distribution};
use crate::filters::{query_param, use_filters};
use crate::format::{ago, group_thousands, short_session_id};

#[derive(Properties, PartialEq)]
pub struct ExceptionDetailProps {
    pub project: String,
    pub group: String,
}

#[function_component(ExceptionDetail)]
pub fn exception_detail(props: &ExceptionDetailProps) -> Html {
    let (project, group) = (props.project.clone(), props.group.clone());
    // The source is part of the group's identity (the same fingerprint on two
    // applications is two independent failures); a link without one is broken.
    let location = use_location();
    let source = location
        .as_ref()
        .and_then(|l| query_param(l.query_str(), "source"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
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
                let Some(source) = source else {
                    return;
                };
                let range = filters.range_query(js_sys::Date::now() as i64);
                spawn_local(async move {
                    detail.set(Some(
                        api::exception_detail(&group, &project, &source, &range).await,
                    ));
                });
            },
        );
    }

    let set_axis = {
        let (project, group, source, reload) = (
            project.clone(),
            group.clone(),
            source.clone(),
            reload.clone(),
        );
        // Build a click handler that flips a single triage axis, leaving the
        // other one untouched (a `None` field is left unchanged server-side).
        move |resolved: Option<bool>, muted: Option<bool>| {
            let input = TriageInput {
                project_id: project.clone(),
                resolved,
                muted,
                note: None,
                source: source.clone().unwrap_or_default(),
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

    let actions = match (&source, &*detail) {
        (Some(_), Some(Ok(detail))) => {
            // Two independent controls: resolution and suppression. Each reflects
            // its own axis, so a group can be (e.g.) resolved *and* muted at once.
            let resolution = if detail.group.resolved {
                html! { <Button icon={icons::UNDO_2} on_click={set_axis(Some(false), None)}>{ "Reopen" }</Button> }
            } else {
                html! { <Button icon={icons::CHECK} on_click={set_axis(Some(true), None)}>{ "Resolve" }</Button> }
            };
            let suppression = if detail.group.muted {
                html! { <Button icon={icons::BELL} on_click={set_axis(None, Some(false))}>{ "Unmute" }</Button> }
            } else {
                html! { <Button icon={icons::BELL_OFF} on_click={set_axis(None, Some(true))}>{ "Mute" }</Button> }
            };
            html! { <ButtonGroup label="Triage">{ resolution }{ suppression }</ButtonGroup> }
        }
        _ => html! {},
    };

    let title = match &*detail {
        Some(Ok(d)) => d.group.exc_type.clone(),
        _ => "Exception".to_string(),
    };
    let subtitle = match &*detail {
        Some(Ok(d)) => html! { { d.group.sample_message.clone() } },
        _ => html! {},
    };
    let meta = match &*detail {
        Some(Ok(d)) => status_pill(d.group.status),
        _ => html! {},
    };
    let breadcrumbs = crumbs("Exceptions", Route::Exceptions, &filters, &title);

    let body = match (&source, &*detail) {
        (None, _) => html! {
            <ApiErrorAlert error={ApiError::Server(
                "This exception link is missing its source. Open the group from the Exceptions inbox.".into()
            )} />
        },
        (Some(_), None) => loading(),
        (Some(_), Some(Err(err))) => html! { <ApiErrorAlert error={err.clone()} /> },
        (Some(source), Some(Ok(detail))) => {
            let summary = format!(
                "{} · {} occurrences · first seen {} · last seen {}",
                source_label(source),
                group_thousands(detail.group.count),
                ago(detail.group.first_seen_ms),
                ago(detail.group.last_seen_ms),
            );
            html! {
                <>
                    <section class="detail-head">
                        <Text tag={TextTag::P} color={TextColor::Muted}>{ summary }</Text>
                        if !detail.group.trend.is_empty() {
                            <div class="detail-head__trend">
                                <Text size={TextSize::Xs} color={TextColor::Subtle} class="eyebrow">
                                    { range_label.clone() }
                                </Text>
                                <Sparkline points={detail.group.trend.clone()} tone={Tone::Danger} />
                            </div>
                        }
                    </section>

                    <section class="section">
                        <header class="section__head">
                            <Heading level={HeadingLevel::H2} size={TextSize::Md}>{ "Distribution" }</Heading>
                        </header>
                        <div class="dist-grid">
                            { distribution("App versions", &detail.breakdowns.app_versions, detail.group.count, Tone::Danger) }
                            { distribution("Applications", &detail.breakdowns.browsers, detail.group.count, Tone::Danger) }
                            { distribution("Operating systems", &detail.breakdowns.operating_systems, detail.group.count, Tone::Danger) }
                            { distribution("Devices", &detail.breakdowns.devices, detail.group.count, Tone::Danger) }
                        </div>
                    </section>

                    <VariantScrubber
                        variants={detail.variants.clone()}
                        key={detail.group.group_id.clone()}
                    />
                    // Pick which of the group's sessions to inspect.
                    <TraceList
                        traces={detail.traces.clone()}
                        hint="Sessions this exception occurred in"
                    />
                </>
            }
        }
    };

    html! {
        <div class="page">
            <PageHeader {title} {subtitle} {breadcrumbs} {meta} {actions} />
            { body }
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct VariantScrubberProps {
    variants: Vec<ExceptionVariant>,
}

/// Scrub through a group's distinct examples; each shows
/// how many identical occurrences it represents.
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
    // The view is scoped to one source, so the reported release stands alone.
    let app = variant
        .app_version
        .as_ref()
        .map(|version| format!("v{version}"));
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
                    attrs={yewi::attrs! { "title" => "Occurrences sharing this exact message and stack trace" }}>
                    { format!("×{} occurrence{}", group_thousands(variant.count), if variant.count == 1 { "" } else { "s" }) }
                </Pill>
                <span>{ ua }</span>
                if let Some(app) = app {
                    <InlineCode>{ app }</InlineCode>
                }
                <Pill dot=false tone={if variant.handled { Tone::Neutral } else { Tone::Warn }}>
                    { if variant.handled { "handled" } else { "unhandled" } }
                </Pill>
                <Text color={TextColor::Subtle}>{ format!("last seen {}", ago(variant.last_seen_ms)) }</Text>
                // Set apart from the descriptive metadata, at the row's end.
                <span class="example__actions">{ trace_link }</span>
            </div>
            <p class="example__message">{ &variant.message }</p>
            if !metadata.tags.is_empty() {
                <div class="example__tags">{ metadata.tag_pills() }</div>
            }
            { metadata.context_list() }
            if let Some(stack) = &variant.stack {
                <CodeBlock code={stack.clone()} wrap=true copy_label="Copy stack trace" />
            }
        </Scrubber>
    }
}
