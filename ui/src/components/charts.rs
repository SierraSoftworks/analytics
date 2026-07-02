//! Hand-rolled interactive SVG charts. The main time-series chart renders in
//! pixel coordinates (the viewBox tracks the measured container size) so text,
//! strokes, and dashes stay undistorted; interactivity is plain Yew events —
//! hover crosshair + tooltip, and click-drag to zoom the time range.

use analytics_api::TimeSeriesPoint;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::HtmlElement;
use yew::prelude::*;

use crate::format::{axis_label, compact, group_thousands, tooltip_label};

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

const HEIGHT: f64 = 280.0;
const MARGIN_TOP: f64 = 14.0;
const MARGIN_RIGHT: f64 = 14.0;
const MARGIN_BOTTOM: f64 = 26.0;
const MARGIN_LEFT: f64 = 46.0;

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

#[function_component(TimeSeriesChart)]
pub fn time_series_chart(props: &TimeSeriesChartProps) -> Html {
    let container = use_node_ref();
    let svg_ref = use_node_ref();
    let width = use_state(|| 0.0f64);
    let hover = use_state(|| None::<usize>);
    // `(anchor, current)` bucket indices of an in-progress drag selection.
    let drag = use_state(|| None::<(usize, usize)>);

    // Track the container width: measure synchronously on mount (ResizeObserver
    // deliveries are frame-timed and may be arbitrarily delayed in throttled
    // tabs), then observe the container so later layout changes — window
    // resizes, sidebar collapses — re-measure automatically.
    {
        let (container, width) = (container.clone(), width.clone());
        use_effect_with((), move |_| {
            let last = std::rc::Rc::new(std::cell::Cell::new(-1.0f64));
            let measure = {
                let (container, width, last) = (container.clone(), width.clone(), last.clone());
                move || {
                    if let Some(el) = container.cast::<HtmlElement>() {
                        let measured = el.client_width() as f64;
                        if (measured - last.get()).abs() >= 1.0 {
                            last.set(measured);
                            width.set(measured);
                        }
                    }
                }
            };
            measure();
            let callback = Closure::<dyn FnMut()>::new(measure);
            let observer = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).ok();
            if let (Some(observer), Some(el)) = (&observer, container.cast::<web_sys::Element>()) {
                observer.observe(&el);
            }
            move || {
                if let Some(observer) = &observer {
                    observer.disconnect();
                }
                drop(callback);
            }
        });
    }

    // Interaction state is meaningless across a data change (a zoom or filter
    // swap can shrink the series); drop it whenever the series length changes.
    {
        let (hover, drag) = (hover.clone(), drag.clone());
        use_effect_with(props.points.len(), move |_| {
            hover.set(None);
            drag.set(None);
            || ()
        });
    }

    let points = &props.points;
    let n = points.len();
    let w = *width;

    if n == 0 {
        return html! {
            <div class="chart" ref={container}>
                <div class="chart__empty">{ "No data in this period." }</div>
            </div>
        };
    }

    let plot_w = (w - MARGIN_LEFT - MARGIN_RIGHT).max(1.0);
    let plot_h = HEIGHT - MARGIN_TOP - MARGIN_BOTTOM;
    let step = if n > 1 { plot_w / (n - 1) as f64 } else { plot_w };
    let x_of = |i: usize| MARGIN_LEFT + if n > 1 { i as f64 * step } else { plot_w / 2.0 };
    let bucket_ms = if n > 1 {
        (points[1].timestamp_ms - points[0].timestamp_ms).max(1)
    } else {
        3_600_000
    };

    // Scale to a "nice" tick ceiling covering both series.
    let metric = props.metric;
    let raw_max = points
        .iter()
        .map(|p| metric.of(p))
        .chain(
            props
                .compare
                .then(|| props.previous.iter().map(|p| metric.of(p)))
                .into_iter()
                .flatten(),
        )
        .max()
        .unwrap_or(0)
        .max(1);
    let (tick_step, tick_count) = nice_ticks(raw_max);
    let y_max = (tick_step * tick_count) as f64;
    let y_of = |v: i64| MARGIN_TOP + (1.0 - v as f64 / y_max) * plot_h;

    // Series geometry.
    let line_path = polyline(points, metric, x_of, y_of);
    let area_path = format!(
        "{line_path} L {:.1} {:.1} L {:.1} {:.1} Z",
        x_of(n - 1),
        MARGIN_TOP + plot_h,
        x_of(0),
        MARGIN_TOP + plot_h,
    );
    let previous_path = (props.compare && !props.previous.is_empty())
        .then(|| polyline(&props.previous, metric, x_of, y_of));

    // Exception occurrences render as red bars rising from the baseline behind
    // the traffic line, on their own scale (capped to the lower portion of the
    // plot so a bad hour reads as a red wall without flattening the line).
    let exc_max = points.iter().map(|p| p.exceptions).max().unwrap_or(0);
    let exception_bars = (exc_max > 0).then(|| {
        let scale = (plot_h * 0.35) / exc_max as f64;
        let bar_w = (step * 0.6).clamp(1.0, 14.0);
        let bars = points.iter().enumerate().filter(|(_, p)| p.exceptions > 0).map(|(i, p)| {
            let h = (p.exceptions as f64 * scale).max(2.0);
            html! {
                <rect key={i.to_string()} class="chart__exception-bar" rx="1"
                    x={format!("{:.1}", x_of(i) - bar_w / 2.0)}
                    y={format!("{:.1}", MARGIN_TOP + plot_h - h)}
                    width={format!("{bar_w:.1}")}
                    height={format!("{h:.1}")} />
            }
        }).collect::<Html>();
        html! { <g>{ bars }</g> }
    });

    // Horizontal gridlines + y labels at each tick.
    let gridlines = (0..=tick_count).map(|t| {
        let value = t * tick_step;
        let y = y_of(value);
        html! {
            <g key={t.to_string()}>
                <line class="chart__grid" x1={MARGIN_LEFT.to_string()} x2={(w - MARGIN_RIGHT).to_string()}
                    y1={format!("{y:.1}")} y2={format!("{y:.1}")} />
                <text class="chart__tick" x={(MARGIN_LEFT - 8.0).to_string()} y={format!("{:.1}", y + 3.5)}
                    text-anchor="end">{ compact(value) }</text>
            </g>
        }
    });

    // X labels: roughly six, aligned to buckets, skipping consecutive repeats
    // (sub-day buckets share a date label within the same day).
    let label_every = (n / 6).max(1);
    let mut last_label = String::new();
    let x_labels = (0..n)
        .step_by(label_every)
        .filter_map(|i| {
            let label = axis_label(points[i].timestamp_ms, bucket_ms);
            if label == last_label {
                return None;
            }
            last_label = label.clone();
            Some(html! {
                <text key={i.to_string()} class="chart__tick" x={format!("{:.1}", x_of(i))}
                    y={(HEIGHT - 8.0).to_string()} text-anchor="middle">
                    { label }
                </text>
            })
        })
        .collect::<Vec<_>>();

    // Pointer interaction: nearest bucket index from a mouse position.
    let index_at = {
        let svg_ref = svg_ref.clone();
        move |e: &MouseEvent| -> Option<usize> {
            let el = svg_ref.cast::<web_sys::Element>()?;
            let rect = el.get_bounding_client_rect();
            let x = e.client_x() as f64 - rect.left();
            let i = ((x - MARGIN_LEFT) / step).round();
            Some((i.max(0.0) as usize).min(n - 1))
        }
    };

    let onmousemove = {
        let (hover, drag) = (hover.clone(), drag.clone());
        let index_at = index_at.clone();
        Callback::from(move |e: MouseEvent| {
            let i = index_at(&e);
            hover.set(i);
            if let (Some((anchor, _)), Some(i)) = (*drag, i)
                && e.buttons() & 1 == 1
            {
                drag.set(Some((anchor, i)));
            }
        })
    };
    let onmousedown = {
        let drag = drag.clone();
        let index_at = index_at.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            if let Some(i) = index_at(&e) {
                drag.set(Some((i, i)));
            }
        })
    };
    let onmouseup = {
        let drag = drag.clone();
        let on_zoom = props.on_zoom.clone();
        let timestamps: Vec<i64> = points.iter().map(|p| p.timestamp_ms).collect();
        Callback::from(move |_: MouseEvent| {
            if let Some((anchor, end)) = *drag {
                let (lo, hi) = (anchor.min(end), anchor.max(end));
                // Bounds-checked: the drag state can be stale after a refetch.
                if hi > lo
                    && let Some(on_zoom) = &on_zoom
                    && let (Some(from), Some(to)) = (timestamps.get(lo), timestamps.get(hi))
                {
                    on_zoom.emit((*from, *to + bucket_ms));
                }
            }
            drag.set(None);
        })
    };
    let onmouseleave = {
        let (hover, drag) = (hover.clone(), drag.clone());
        Callback::from(move |_: MouseEvent| {
            hover.set(None);
            drag.set(None);
        })
    };

    // Drag-selection shading. Stale indices (state persisted across a data
    // change) are clamped rather than trusted — indexing with them panics.
    let selection = (*drag).map(|(anchor, end)| {
        let (lo, hi) = (anchor.min(end).min(n - 1), anchor.max(end).min(n - 1));
        html! {
            <rect class="chart__selection" x={format!("{:.1}", x_of(lo))} y={MARGIN_TOP.to_string()}
                width={format!("{:.1}", x_of(hi) - x_of(lo))} height={plot_h.to_string()} />
        }
    });

    // Hover crosshair, markers, and the HTML tooltip. `hover` may briefly hold
    // an index from a longer series (state outlives a refetch), so look up
    // defensively instead of indexing.
    let hover_bits = (*hover).and_then(|i| points.get(i).map(|point| (i, point))).map(|(i, point)| {
        let x = x_of(i);
        let value = metric.of(point);
        let prev = (props.compare)
            .then(|| props.previous.get(i).map(|p| metric.of(p)))
            .flatten();
        let marker_y = y_of(value);

        // Flip the tooltip to the left of the crosshair in the right half.
        let tooltip_style = if x > w / 2.0 {
            format!("right: {:.0}px; top: {:.0}px;", w - x + 12.0, MARGIN_TOP + 8.0)
        } else {
            format!("left: {:.0}px; top: {:.0}px;", x + 12.0, MARGIN_TOP + 8.0)
        };

        let secondary = match metric {
            Metric::Visitors => format!("{} page views", group_thousands(point.pageviews)),
            _ => format!("{} visitors", group_thousands(point.visitors)),
        };

        (
            html! {
                <g>
                    <line class="chart__crosshair" x1={format!("{x:.1}")} x2={format!("{x:.1}")}
                        y1={MARGIN_TOP.to_string()} y2={(MARGIN_TOP + plot_h).to_string()} />
                    <circle class="chart__marker" cx={format!("{x:.1}")} cy={format!("{marker_y:.1}")} r="3.5" />
                </g>
            },
            html! {
                <div class="chart__tooltip" style={tooltip_style}>
                    <div class="chart__tooltip-date">{ tooltip_label(point.timestamp_ms, bucket_ms) }</div>
                    <div class="chart__tooltip-row">
                        <span class="chart__tooltip-dot" />
                        <span>{ metric.label() }</span>
                        <strong>{ group_thousands(value) }</strong>
                    </div>
                    if let Some(prev) = prev {
                        <div class="chart__tooltip-row chart__tooltip-row--prev">
                            <span class="chart__tooltip-dot chart__tooltip-dot--prev" />
                            <span>{ "Previous" }</span>
                            <strong>{ group_thousands(prev) }</strong>
                        </div>
                    }
                    if point.exceptions > 0 {
                        <div class="chart__tooltip-row chart__tooltip-row--exceptions">
                            <span class="chart__tooltip-dot chart__tooltip-dot--exceptions" />
                            <span>{ "Exceptions" }</span>
                            <strong>{ group_thousands(point.exceptions) }</strong>
                        </div>
                    }
                    <div class="chart__tooltip-sub">{ secondary }</div>
                </div>
            },
        )
    });
    let (hover_svg, tooltip) = match hover_bits {
        Some((s, t)) => (Some(s), Some(t)),
        None => (None, None),
    };

    html! {
        <div class="chart" ref={container}>
            if w > 0.0 {
                <svg
                    ref={svg_ref}
                    viewBox={format!("0 0 {w:.0} {HEIGHT:.0}")}
                    width="100%"
                    height={HEIGHT.to_string()}
                    role="img"
                    aria-label={format!("{} over time", metric.label())}
                    {onmousemove}
                    {onmousedown}
                    {onmouseup}
                    {onmouseleave}
                >
                    <defs>
                        <linearGradient id="chart-fill" x1="0" y1="0" x2="0" y2="1">
                            <stop offset="0" class="chart__fill-start" />
                            <stop offset="1" class="chart__fill-end" />
                        </linearGradient>
                    </defs>
                    { for gridlines }
                    { for x_labels }
                    { exception_bars }
                    <path class="chart__area" d={area_path} fill="url(#chart-fill)" />
                    if let Some(previous_path) = previous_path {
                        <path class="chart__line chart__line--prev" d={previous_path} />
                    }
                    <path class="chart__line" d={line_path} />
                    { selection }
                    { hover_svg }
                </svg>
                { tooltip }
            }
        </div>
    }
}

/// The `M … L …` polyline through a series in plot coordinates.
fn polyline(
    points: &[TimeSeriesPoint],
    metric: Metric,
    x_of: impl Fn(usize) -> f64,
    y_of: impl Fn(i64) -> f64,
) -> String {
    let mut path = String::with_capacity(points.len() * 16);
    for (i, point) in points.iter().enumerate() {
        let command = if i == 0 { 'M' } else { 'L' };
        path.push_str(&format!(
            "{command} {:.1} {:.1} ",
            x_of(i),
            y_of(metric.of(point))
        ));
    }
    path.trim_end().to_string()
}

/// A tick step and count whose product comfortably covers `max` (1-2-5 ladder).
fn nice_ticks(max: i64) -> (i64, i64) {
    const COUNT: i64 = 4;
    let raw = (max as f64 / COUNT as f64).max(0.25);
    let magnitude = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .into_iter()
        .map(|s| s * magnitude)
        .find(|s| s * COUNT as f64 >= max as f64)
        .unwrap_or(10.0 * magnitude);
    let step = (step.round() as i64).max(1);
    let count = ((max + step - 1) / step).max(1);
    (step, count)
}

#[derive(Properties, PartialEq)]
pub struct SparklineProps {
    pub points: Vec<i64>,
    #[prop_or_default]
    pub class: Classes,
}

/// A dependency-free mini trend line for table rows and cards. No text lives
/// inside, so it can safely stretch with `preserveAspectRatio="none"`; the
/// stroke stays uniform via `vector-effect`.
#[function_component(Sparkline)]
pub fn sparkline(props: &SparklineProps) -> Html {
    const W: f64 = 100.0;
    const H: f64 = 28.0;
    const PAD: f64 = 2.0;

    let points = &props.points;
    if points.is_empty() || points.iter().all(|v| *v == 0) {
        return html! { <svg class={classes!("sparkline", props.class.clone())} viewBox={format!("0 0 {W} {H}")} preserveAspectRatio="none" aria-hidden="true">
            <line class="sparkline__flat" x1="0" y1={(H - PAD).to_string()} x2={W.to_string()} y2={(H - PAD).to_string()} />
        </svg> };
    }

    let max = points.iter().copied().max().unwrap_or(1).max(1) as f64;
    let step = if points.len() > 1 { W / (points.len() - 1) as f64 } else { W };
    let coords: Vec<(f64, f64)> = points
        .iter()
        .enumerate()
        .map(|(i, v)| (i as f64 * step, PAD + (1.0 - *v as f64 / max) * (H - 2.0 * PAD)))
        .collect();
    let line = coords
        .iter()
        .enumerate()
        .map(|(i, (x, y))| format!("{} {x:.1} {y:.1}", if i == 0 { "M" } else { "L" }))
        .collect::<Vec<_>>()
        .join(" ");
    let area = format!("{line} L {W} {H} L 0 {H} Z");

    html! {
        <svg class={classes!("sparkline", props.class.clone())} viewBox={format!("0 0 {W} {H}")}
            preserveAspectRatio="none" aria-hidden="true">
            <path class="sparkline__area" d={area} />
            <path class="sparkline__line" d={line} />
        </svg>
    }
}
