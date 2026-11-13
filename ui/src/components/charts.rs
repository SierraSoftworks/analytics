//! The dashboard's charts, drawn by yewi's `Chart`: the traffic series with its
//! previous-period overlay, hover tooltip and drag-to-zoom, and the exception
//! counts as a bar strip beneath it.

use analytics_api::TimeSeriesPoint;
use yew::prelude::*;
use yewi::prelude::{
    Chart, Series, SeriesColor, SeriesDash, SeriesData, SeriesKind, Sparkline as YiSparkline, Tone,
};

use crate::format::chart_label;

/// The metric a chart (and the breakdown panels) currently displays.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Metric {
    #[default]
    Visitors,
    Pageviews,
    Events,
}

impl Metric {
    pub fn label(self) -> &'static str {
        match self {
            Metric::Visitors => "Visitors",
            Metric::Pageviews => "Page views",
            Metric::Events => "Events",
        }
    }

    pub fn of(self, point: &TimeSeriesPoint) -> i64 {
        match self {
            Metric::Visitors => point.visitors,
            Metric::Pageviews => point.pageviews,
            Metric::Events => point.events,
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct TimeSeriesChartProps {
    pub points: Vec<TimeSeriesPoint>,
    /// Previous-window series, index-aligned with `points` (may be empty).
    #[prop_or_default]
    pub previous: Vec<TimeSeriesPoint>,
    #[prop_or_default]
    pub metric: Metric,
    /// Overlay the previous window as a dashed line.
    #[prop_or(false)]
    pub compare: bool,
    /// Emitted with `(from_ms, to_ms)` when the user drag-selects a sub-range.
    #[prop_or_default]
    pub on_zoom: Option<Callback<(i64, i64)>>,
}

/// The series a chart draws, and the exception counts when there are any.
/// Built once per dataset: `SeriesData` compares by pointer, so the charts
/// only recompute their geometry when this is rebuilt.
struct Prepared {
    traffic: Vec<Series>,
    exceptions: Option<Vec<Series>>,
}

#[function_component(TimeSeriesChart)]
pub fn time_series_chart(props: &TimeSeriesChartProps) -> Html {
    let prepared = use_memo(
        (
            props.points.clone(),
            props.previous.clone(),
            props.metric,
            props.compare,
        ),
        |(points, previous, metric, compare)| {
            let values = |points: &[TimeSeriesPoint]| -> SeriesData {
                points.iter().map(|p| metric.of(p) as f64).collect()
            };
            let mut traffic = vec![Series::new(metric.label(), values(points))];
            if *compare && !previous.is_empty() {
                traffic.push(
                    Series::new("Previous period", values(previous))
                        .color(SeriesColor::Muted)
                        .dash(SeriesDash::Dashed),
                );
            }
            // Exceptions sit on a strip of their own: on the traffic scale a
            // bad hour of a few dozen failures would be a flat line.
            let exceptions = points.iter().any(|p| p.exceptions > 0).then(|| {
                let data: SeriesData = points.iter().map(|p| p.exceptions as f64).collect();
                vec![
                    Series::new("Exceptions", data)
                        .kind(SeriesKind::Bars)
                        .color(SeriesColor::Tone(Tone::Danger)),
                ]
            });
            Prepared {
                traffic,
                exceptions,
            }
        },
    );

    let timestamps: std::rc::Rc<[i64]> = props.points.iter().map(|p| p.timestamp_ms).collect();
    let bucket_ms = match &*timestamps {
        [first, second, ..] => (second - first).max(1),
        _ => 3_600_000,
    };
    let x_label = {
        let timestamps = timestamps.clone();
        Callback::from(move |i: usize| -> AttrValue {
            timestamps
                .get(i)
                .map(|ms| chart_label(*ms, bucket_ms))
                .unwrap_or_default()
                .into()
        })
    };
    let on_select = {
        let (timestamps, on_zoom) = (timestamps.clone(), props.on_zoom.clone());
        Callback::from(move |(lo, hi): (usize, usize)| {
            // Bounds-checked: a selection can outlive the data it was made on.
            if hi > lo
                && let Some(on_zoom) = &on_zoom
                && let (Some(from), Some(to)) = (timestamps.get(lo), timestamps.get(hi))
            {
                on_zoom.emit((*from, *to + bucket_ms));
            }
        })
    };

    // The traffic chart takes bands when the exception strip is shown, so a
    // position in one sits over the same position in the other.
    let banded = prepared.exceptions.is_some().then_some(true);

    html! {
        <div class="traffic-chart">
            <Chart
                series={prepared.traffic.clone()}
                label={format!("{} over time", props.metric.label())}
                x_label={x_label.clone()}
                y_label={whole_ticks()}
                legend=false
                selectable={props.on_zoom.is_some()}
                {banded}
                {on_select}
                empty="No data in this period."
            />
            if let Some(exceptions) = &prepared.exceptions {
                <span class="traffic-chart__caption">{ "Exceptions" }</span>
                <Chart
                    class="traffic-chart__exceptions"
                    series={exceptions.clone()}
                    label="Exceptions over time"
                    {x_label}
                    y_label={whole_ticks()}
                    legend=false
                />
            }
        </div>
    }
}

#[derive(Properties, PartialEq)]
pub struct SparklineProps {
    pub points: Vec<i64>,
    /// The status the trend carries; the accent when it carries none.
    #[prop_or_default]
    pub tone: Option<Tone>,
    #[prop_or_default]
    pub class: Classes,
}

/// A mini trend line for list rows and detail headers.
#[function_component(Sparkline)]
pub fn sparkline(props: &SparklineProps) -> Html {
    let data = use_memo(props.points.clone(), |points| -> SeriesData {
        points.iter().map(|v| *v as f64).collect()
    });
    let color = props.tone.map_or(SeriesColor::Auto, SeriesColor::Tone);
    html! { <YiSparkline data={(*data).clone()} {color} class={props.class.clone()} /> }
}

/// Labels whole numbers only: a count axis has no use for a "0.5" tick.
fn whole_ticks() -> Callback<f64, AttrValue> {
    Callback::from(|value: f64| -> AttrValue {
        if value.fract() == 0.0 {
            crate::format::group_thousands(value as i64).into()
        } else {
            AttrValue::default()
        }
    })
}
