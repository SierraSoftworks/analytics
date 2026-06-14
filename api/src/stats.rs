use serde::{Deserialize, Serialize};

use crate::project::Project;

/// Headline metrics for a time range over a set of sources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricSummary {
    /// Sum of daily-unique visitors over the range.
    pub visitors: i64,
    pub pageviews: i64,
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
}

/// A single breakdown row (e.g. a page path and its view count).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyCount {
    pub key: String,
    pub count: i64,
}

/// Full statistics for a project (or a filtered subset of its sources).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub summary: MetricSummary,
    pub timeseries: Vec<TimeSeriesPoint>,
    pub pages: Vec<KeyCount>,
    pub referrers: Vec<KeyCount>,
    pub browsers: Vec<KeyCount>,
    pub operating_systems: Vec<KeyCount>,
    pub devices: Vec<KeyCount>,
    pub countries: Vec<KeyCount>,
    pub languages: Vec<KeyCount>,
    pub sources: Vec<KeyCount>,
}

/// Per-project totals shown on the global overview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub project: Project,
    pub visitors: i64,
    pub pageviews: i64,
}

/// Totals for a single source URI on the global overview (used for sources not yet
/// assigned to a project).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSummary {
    pub uri: String,
    pub visitors: i64,
    pub pageviews: i64,
}

/// The global overview across all projects (plus any unassigned sources).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Overview {
    pub summary: MetricSummary,
    pub timeseries: Vec<TimeSeriesPoint>,
    pub projects: Vec<ProjectSummary>,
    /// Sources not yet assigned to a project, each with its visitor and view totals.
    pub unassigned: Vec<SourceSummary>,
}
