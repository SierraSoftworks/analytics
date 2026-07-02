use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The triage status of an exception group, set by an administrator. Stored
/// separately from the (append-only) occurrence stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExceptionStatus {
    #[default]
    Unresolved,
    Resolved,
    Ignored,
}

/// The payload posted to `/track/exception` by the tracking script (short keys to
/// keep the beacon small).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionReport {
    /// The page URL the exception occurred on (used to attribute it to a source).
    #[serde(rename = "u")]
    pub url: String,
    #[serde(rename = "b", default, skip_serializing_if = "Option::is_none")]
    pub beacon: Option<String>,
    /// The exception type/name (e.g. "TypeError").
    #[serde(rename = "ty")]
    pub exc_type: String,
    #[serde(rename = "m")]
    pub message: String,
    #[serde(rename = "s", default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    /// Whether the exception was handled (vs an unhandled error/rejection).
    #[serde(rename = "h", default)]
    pub handled: bool,
    /// The reporting application's version (the tracker's `data-app-version`).
    /// The application itself is identified by the report's hostname — the
    /// same `source` attribution every other event uses.
    #[serde(rename = "v", default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    /// An optional client-supplied grouping fingerprint override.
    #[serde(rename = "fp", default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(rename = "d", default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, String>>,
}

/// The number of buckets in an exception group's occurrence [`ExceptionGroup::trend`].
pub const TREND_BUCKETS: usize = 20;

/// An aggregated group of exception occurrences sharing a fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionGroup {
    pub group_id: String,
    pub exc_type: String,
    pub sample_message: String,
    pub count: i64,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    pub status: ExceptionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Occurrence counts over the query range, split into [`TREND_BUCKETS`]
    /// equal buckets (oldest first), for the frequency sparkline.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trend: Vec<i64>,
}

/// A distinct example within an exception group: occurrences sharing the same
/// message, stack trace, and handledness, collapsed into one representative
/// with a count. The detail page scrubs through these rather than paging a
/// flat occurrence list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionVariant {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    pub handled: bool,
    /// How many occurrences share this exact message/stack/handledness.
    pub count: i64,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    /// Client context from the most recent occurrence of this variant. The
    /// source doubles as the application identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ua_browser: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ua_os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    /// Reporter-supplied metadata as a raw JSON object string, from the most
    /// recent occurrence of this variant that carried any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
}

/// How a group's occurrences distribute across key dimensions (empty keys mean
/// the dimension was absent on those occurrences). Applications are identified
/// by source, so the sources distribution *is* the per-app distribution.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExceptionBreakdowns {
    pub app_versions: Vec<crate::CountRow>,
    pub browsers: Vec<crate::CountRow>,
    pub operating_systems: Vec<crate::CountRow>,
    pub devices: Vec<crate::CountRow>,
    pub sources: Vec<crate::CountRow>,
}

/// An exception group with its dimension distributions and distinct examples.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionGroupDetail {
    pub group: ExceptionGroup,
    #[serde(default)]
    pub breakdowns: ExceptionBreakdowns,
    pub variants: Vec<ExceptionVariant>,
}

/// An exception group annotated with the project it belongs to, for the global
/// Exceptions inbox. `project_id`/`project_name` are absent when the originating
/// source is unassigned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalException {
    pub group: ExceptionGroup,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    /// A representative source URI the group was seen on.
    pub source: String,
}

/// Payload for updating an exception group's triage state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageInput {
    pub project_id: String,
    pub status: ExceptionStatus,
    #[serde(default)]
    pub note: Option<String>,
}
