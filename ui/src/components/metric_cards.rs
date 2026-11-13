//! The headline metric tiles: value, delta vs the previous window, and (for the
//! countable metrics) selection of which metric the chart and panels display.

use analytics_api::MetricSummary;
use yew::prelude::*;
use yewi::prelude::{StatTile, StatTrend};

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
        let on_click = {
            let on_metric = props.on_metric.clone();
            Callback::from(move |_: MouseEvent| on_metric.emit(metric))
        };
        let (delta, trend) = delta_parts(delta_percent(value as f64, previous as f64));
        html! {
            <StatTile
                label={metric.label()}
                value={group_thousands(value)}
                {delta} {trend}
                detail="vs. previous period"
                selected={props.metric == metric}
                {on_click}
            />
        }
    };

    // Only surface the events tile when pixels/custom events exist at all.
    let show_events = s.events > 0 || p.events > 0 || props.metric == Metric::Events;
    let (bounce_delta, bounce_trend) = delta_parts(bounce_delta(s, p));
    let (duration_delta, duration_trend) = delta_parts(duration_delta(s, p));

    html! {
        <div class="stats">
            { selectable(Metric::Visitors, s.visitors, p.visitors) }
            { selectable(Metric::Pageviews, s.pageviews, p.pageviews) }
            if show_events {
                { selectable(Metric::Events, s.events, p.events) }
            }
            <StatTile label="Bounce rate" value={bounce} delta={bounce_delta} trend={bounce_trend}
                lower_is_better=true detail="vs. previous period" />
            <StatTile label="Median time" value={duration} delta={duration_delta} trend={duration_trend}
                detail="vs. previous period" />
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

/// The change since the previous window as StatTile's `delta` text and
/// `trend` direction. Changes under 0.05% read as flat.
fn delta_parts(delta: Option<f64>) -> (Option<AttrValue>, Option<StatTrend>) {
    match delta {
        None => (None, None),
        Some(delta) => (
            Some(format!("{:.0}%", delta.abs()).into()),
            Some(StatTrend::of(delta, 0.05)),
        ),
    }
}
