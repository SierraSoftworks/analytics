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
    /// An optional client-supplied grouping fingerprint override.
    #[serde(rename = "fp", default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(rename = "d", default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, String>>,
}

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
}

/// A single exception occurrence within a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionOccurrence {
    pub exc_type: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    pub handled: bool,
    pub received_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ua_browser: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ua_os: Option<String>,
}

/// An exception group with its recent occurrences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionGroupDetail {
    pub group: ExceptionGroup,
    pub occurrences: Vec<ExceptionOccurrence>,
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
