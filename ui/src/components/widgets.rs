use analytics_api::{KeyCount, MetricSummary, TimeSeriesPoint};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct MetricCardsProps {
    pub summary: MetricSummary,
}

/// The four headline metrics, rendered as Element-Plus-style statistic cards.
#[function_component(MetricCards)]
pub fn metric_cards(props: &MetricCardsProps) -> Html {
    let s = &props.summary;
    let bounce = s
        .bounce_rate
        .map(|b| format!("{:.0}%", b * 100.0))
        .unwrap_or_else(|| "—".to_string());
    let duration = s
        .median_duration_ms
        .map(format_duration)
        .unwrap_or_else(|| "—".to_string());

    html! {
        <div class="stats">
            { stat("Visitors", &group_thousands(s.visitors)) }
            { stat("Page views", &group_thousands(s.pageviews)) }
            { stat("Bounce rate", &bounce) }
            { stat("Median time", &duration) }
        </div>
    }
}

fn stat(label: &str, value: &str) -> Html {
    html! {
        <div class="stat">
            <span class="stat__label">{ label }</span>
            <span class="stat__value">{ value }</span>
        </div>
    }
}

/// Group an integer with thin thousands separators, e.g. `12,345`.
fn group_thousands(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn format_duration(ms: i64) -> String {
    if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

#[derive(Properties, PartialEq)]
pub struct TimeSeriesChartProps {
    pub points: Vec<TimeSeriesPoint>,
}

#[function_component(TimeSeriesChart)]
pub fn time_series_chart(props: &TimeSeriesChartProps) -> Html {
    let points = &props.points;
    if points.is_empty() {
        return html! { <p class="muted">{ "No data for this period." }</p> };
    }

    let max = points.iter().map(|p| p.pageviews).max().unwrap_or(1).max(1) as f64;
    let slot = 100.0 / points.len() as f64;
    // Cap the bar width and centre it in its slot so a single (or few) data point(s)
    // render as discrete bars rather than one block stretched across the whole chart.
    const MAX_BAR: f64 = 7.0;
    let bar_w = (slot * 0.8).min(MAX_BAR);

    let bars = points.iter().enumerate().map(|(i, p)| {
        let height = (p.pageviews as f64 / max) * 100.0;
        let x = i as f64 * slot + (slot - bar_w) / 2.0;
        html! {
            <rect class="bar" rx="0.4"
                x={format!("{:.3}", x)}
                y={format!("{:.3}", 100.0 - height)}
                width={format!("{:.3}", bar_w)}
                height={format!("{:.3}", height)}>
                <title>{ format!("{} views · {} visitors", p.pageviews, p.visitors) }</title>
            </rect>
        }
    });

    html! {
        <svg class="chart" viewBox="0 0 100 100" preserveAspectRatio="none">
            { for bars }
        </svg>
    }
}

#[derive(Properties, PartialEq)]
pub struct BreakdownProps {
    pub title: AttrValue,
    pub rows: Vec<KeyCount>,
}

#[function_component(Breakdown)]
pub fn breakdown(props: &BreakdownProps) -> Html {
    html! {
        <div class="breakdown">
            <h3>{ props.title.clone() }</h3>
            if props.rows.is_empty() {
                <p class="muted">{ "No data." }</p>
            } else {
                <table aria-label={props.title.clone()}>
                    { for props.rows.iter().map(|row| html! {
                        <tr>
                            <td class="breakdown__key" title={row.key.clone()}>{ &row.key }</td>
                            <td class="breakdown__count">{ row.count }</td>
                        </tr>
                    }) }
                </table>
            }
        </div>
    }
}
