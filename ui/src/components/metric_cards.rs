//! The headline metric cards: value, delta vs the previous window, and (for the
//! countable metrics) selection of which metric the chart and panels display.

use analytics_api::MetricSummary;
use yew::prelude::*;

use crate::components::charts::Metric;
use crate::format::{delta_percent, format_duration, group_thousands};

#[derive(Properties, PartialEq)]
pub struct MetricCardsProps {
    pub summary: MetricSummary,
    pub previous: MetricSummary,
    pub metric: Metric,
    pub on_metric: Callback<Metric>,
}

#[function_component(MetricCards)]
pub fn metric_cards(props: &MetricCardsProps) -> Html {
    let s = &props.summary;
    let p = &props.previous;

    let bounce = s
        .bounce_rate
        .map(|b| format!("{:.0}%", b * 100.0))
        .unwrap_or_else(|| "—".to_string());
    let duration = s
        .median_duration_ms
        .map(format_duration)
        .unwrap_or_else(|| "—".to_string());

    let selectable = |metric: Metric, value: i64, previous: i64| {
        let active = props.metric == metric;
        let onclick = {
            let on_metric = props.on_metric.clone();
            Callback::from(move |_: MouseEvent| on_metric.emit(metric))
        };
        let delta = delta_percent(value as f64, previous as f64);
        html! {
            <button
                class={classes!("stat", "stat--selectable", active.then_some("stat--active"))}
                onclick={onclick}
                aria-pressed={active.to_string()}
            >
                <span class="stat__label">{ metric.label() }</span>
                <span class="stat__value">{ group_thousands(value) }</span>
                { delta_badge(delta, false) }
            </button>
        }
    };

    // Only surface the events card when pixels/custom events exist at all.
    let show_events = s.events > 0 || p.events > 0 || props.metric == Metric::Events;

    html! {
        <div class={classes!("stats", show_events.then_some("stats--five"))}>
            { selectable(Metric::Visitors, s.visitors, p.visitors) }
            { selectable(Metric::Pageviews, s.pageviews, p.pageviews) }
            if show_events {
                { selectable(Metric::Events, s.events, p.events) }
            }
            <div class="stat">
                <span class="stat__label">{ "Bounce rate" }</span>
                <span class="stat__value">{ bounce }</span>
                { delta_badge(bounce_delta(s, p), true) }
            </div>
            <div class="stat">
                <span class="stat__label">{ "Median time" }</span>
                <span class="stat__value">{ duration }</span>
                { delta_badge(duration_delta(s, p), false) }
            </div>
        </div>
    }
}

fn bounce_delta(s: &MetricSummary, p: &MetricSummary) -> Option<f64> {
    match (s.bounce_rate, p.bounce_rate) {
        (Some(current), Some(previous)) => delta_percent(current, previous),
        _ => None,
    }
}

fn duration_delta(s: &MetricSummary, p: &MetricSummary) -> Option<f64> {
    match (s.median_duration_ms, p.median_duration_ms) {
        (Some(current), Some(previous)) => delta_percent(current as f64, previous as f64),
        _ => None,
    }
}

/// The small ▲/▼ percentage badge. `invert` flips the good/bad colouring for
/// metrics where up is worse (bounce rate).
fn delta_badge(delta: Option<f64>, invert: bool) -> Html {
    let Some(delta) = delta else {
        return html! { <span class="stat__delta stat__delta--none">{ "—" }</span> };
    };
    if delta.abs() < 0.05 {
        return html! { <span class="stat__delta stat__delta--none">{ "±0%" }</span> };
    }
    let up = delta > 0.0;
    let good = up != invert;
    let class = classes!(
        "stat__delta",
        if good {
            "stat__delta--good"
        } else {
            "stat__delta--bad"
        }
    );
    let arrow = if up { "▲" } else { "▼" };
    html! {
        <span class={class} title="vs. previous period">
            { format!("{arrow} {:.0}%", delta.abs()) }
        </span>
    }
}
