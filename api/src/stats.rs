use serde::{Deserialize, Serialize};

/// Query parameters accepted by `GET /api/v1/stats`.
///
/// Filtering is expressed as a single [`filt-rs`] expression in `q` — e.g.
/// `browser == "Chrome" && (country == "DE" || country == "AT")` — over the
/// event dimensions (`source`, `project`, `path`, `referrer`, `country`,
/// `language`, `browser`, `os`, `device`, `utm_source`, `utm_medium`,
/// `utm_campaign`). Comparing a dimension to the **empty string** matches
/// events where it is absent (direct traffic, unknown country, …) so sentinel
/// breakdown rows round-trip as filters.
///
/// [`filt-rs`]: https://github.com/SierraSoftworks/filters
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DashboardQuery {
    /// Range start (epoch millis, inclusive); defaults to 7 days before `to`.
    /// `0` means "all time": the server anchors the window at its earliest
    /// stored event instead of the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    /// Range end (epoch millis, exclusive); defaults to now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
    /// Time-series bucket: `minute` | `15m` | `hour` | `4h` | `6h` | `day` |
    /// `week`. Defaults to `day`; the server coarsens it if the range would
    /// produce an unreasonable number of buckets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    /// The filter expression; absent or blank means unfiltered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
}

/// Headline metrics for a time range over a set of sources.
///
/// `visitors` counts daily-unique visitors (the tracker's `is_unique_user`
/// flag, which rides only on the first page load of a visitor's day). When the
/// query filters by `path`, the flag would undercount badly (non-landing pages
/// would show ~zero), so visitors are counted from `is_unique_page` instead —
/// daily-unique views of that page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricSummary {
    pub visitors: i64,
    pub pageviews: i64,
    /// Pixel hits and custom application events. Dimension columns are null on
    /// these events, so any dimension filter naturally excludes them.
    #[serde(default)]
    pub events: i64,
    /// Fraction (0..1) of measured visits that bounced; `None` with too few samples.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounce_rate: Option<f64>,
    /// Median time on page, in milliseconds; `None` if unmeasured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_duration_ms: Option<i64>,
}

/// One point in a metrics time series (bucket start, in epoch millis).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSeriesPoint {
    pub timestamp_ms: i64,
    pub visitors: i64,
    pub pageviews: i64,
    /// Pixel + custom events in this bucket.
    #[serde(default)]
    pub events: i64,
    /// Exception occurrences in this bucket (the chart's red underlay bars).
    #[serde(default)]
    pub exceptions: i64,
}

/// A plain `key → count` row (e.g. one slice of an exception-dimension
/// distribution). An empty `key` means the dimension was absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountRow {
    pub key: String,
    pub count: i64,
}

/// One row of a dimension breakdown.
///
/// An **empty `key`** aggregates events where the dimension is absent (direct
/// traffic, unknown country, no campaign, …); the UI renders it as a sentinel
/// ("Direct", "Unknown") and it filters via an empty query-param value.
///
/// `visitors` is daily-unique visitors for visitor-stable dimensions and
/// daily-unique *page* views for the pages breakdown; referrer/UTM rows carry
/// landing-page attribution (the flag rides on the first load of the day).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakdownRow {
    pub key: String,
    pub visitors: i64,
    pub pageviews: i64,
    /// Pixel + custom events (only ever non-zero for the project/source/
    /// unassigned breakdowns; dimension columns are null on such events).
    #[serde(default)]
    pub events: i64,
}

/// One row of the client-versions breakdown. A version number is only
/// meaningful within its application ("120.0" from Chrome and "120.0" from
/// Edge are unrelated), so rows are keyed by the (application, version) pair
/// rather than the bare version string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRow {
    /// The UA-derived application (browser or client app) name; empty when unknown.
    pub app: String,
    /// The version within that application; empty when unknown.
    pub version: String,
    pub visitors: i64,
    pub pageviews: i64,
    #[serde(default)]
    pub events: i64,
}

/// Every dashboard breakdown, computed over the same filtered event set so the
/// panels agree with the headline metrics and with each other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breakdowns {
    pub pages: Vec<BreakdownRow>,
    pub referrers: Vec<BreakdownRow>,
    pub countries: Vec<BreakdownRow>,
    pub languages: Vec<BreakdownRow>,
    /// UA-derived applications: browsers and client apps alike.
    pub browsers: Vec<BreakdownRow>,
    /// UA-derived client versions, one row per (application, version) pair.
    /// `serde(default)` tolerates payloads from agents predating the column.
    #[serde(default)]
    pub versions: Vec<VersionRow>,
    pub operating_systems: Vec<BreakdownRow>,
    pub devices: Vec<BreakdownRow>,
    pub utm_sources: Vec<BreakdownRow>,
    pub utm_mediums: Vec<BreakdownRow>,
    pub utm_campaigns: Vec<BreakdownRow>,
    /// Keyed by project id (the UI maps ids to names). Includes pixel and
    /// custom events in `events`.
    pub projects: Vec<BreakdownRow>,
    /// Keyed by source URI. Includes pixel and custom events in `events`.
    pub sources: Vec<BreakdownRow>,
}

/// The full dashboard payload for `GET /api/v1/stats`: one round trip serves
/// the global view and every filtered drill-down of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dashboard {
    pub summary: MetricSummary,
    /// The same metrics over the immediately-preceding window of equal length
    /// (for the delta badges on the metric cards).
    pub previous_summary: MetricSummary,
    pub timeseries: Vec<TimeSeriesPoint>,
    /// Previous-window series, **index-aligned** with `timeseries`: same
    /// length, bucket `i` compares with bucket `i` (timestamps are the
    /// previous window's own instants).
    pub previous_timeseries: Vec<TimeSeriesPoint>,
    pub breakdowns: Breakdowns,
    /// Per-source totals for traffic not assigned to any project (keyed by
    /// source URI), driving the operator "assign these sources" flow. Derived
    /// from events, so it also surfaces sources beyond the auto-registration
    /// cap.
    pub unassigned: Vec<BreakdownRow>,
}
