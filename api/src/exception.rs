use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The collapsed, display-oriented triage status of an exception group, derived
/// from the two independent triage axes (resolution and suppression). Muting takes
/// precedence, so a muted group reads as `Ignored` regardless of whether it is also
/// resolved; the raw axes travel alongside it on [`ExceptionGroup`] for controls
/// that need to act on each independently (see [`ExceptionStatus::from`]).
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
    /// The per-visit session id (see [`crate::TrackEvent::session`]), linking
    /// the report to the visit's page views. Same `i` key as on hits (`s` is
    /// taken by the stack here).
    #[serde(rename = "i", default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
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

/// The first non-empty line of an exception message, trimmed.
///
/// Exception messages frequently lead with a short one-line summary followed by
/// several lines of diagnostic context. That summary is what groups are
/// fingerprinted on and what list rows and detail headings surface; the full
/// message (context and all) is preserved on the stored occurrence and shown in
/// the distinct-example exemplars.
pub fn summary_line(message: &str) -> &str {
    message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
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
    /// The collapsed display status ([`ExceptionStatus::derive`] of the two axes
    /// below), for the inbox badge and status tabs.
    pub status: ExceptionStatus,
    /// Whether the group is currently resolved. A group resolved in the past but
    /// seen again since counts as unresolved here (a regression reopens it).
    #[serde(default)]
    pub resolved: bool,
    /// Whether the group is suppressed. Orthogonal to `resolved`.
    #[serde(default)]
    pub muted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Occurrence counts over the query range, split into [`TREND_BUCKETS`]
    /// equal buckets (oldest first), for the frequency sparkline.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trend: Vec<i64>,
}

/// Collapse a group's two orthogonal triage axes into its single display status.
/// Suppression wins: a muted group is `Ignored` even when it is also resolved.
impl From<&ExceptionGroup> for ExceptionStatus {
    fn from(group: &ExceptionGroup) -> Self {
        if group.muted {
            Self::Ignored
        } else if group.resolved {
            Self::Resolved
        } else {
            Self::Unresolved
        }
    }
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
    /// The session the most recent session-linked occurrence belonged to,
    /// linking the exemplar to its session trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// How a group's occurrences distribute across key dimensions (empty keys mean
/// the dimension was absent on those occurrences). The detail view is scoped
/// to one source, so there is no per-source distribution — the source is part
/// of the group's identity.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExceptionBreakdowns {
    /// Reported releases, keyed by bare version number (the application is
    /// given by the view's source). Versionless occurrences aggregate under
    /// the empty sentinel.
    pub app_versions: Vec<crate::CountRow>,
    pub browsers: Vec<crate::CountRow>,
    pub operating_systems: Vec<crate::CountRow>,
    pub devices: Vec<crate::CountRow>,
}

/// An exception group with its dimension distributions and distinct examples.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionGroupDetail {
    pub group: ExceptionGroup,
    #[serde(default)]
    pub breakdowns: ExceptionBreakdowns,
    pub variants: Vec<ExceptionVariant>,
    /// The most recent sessions the group's occurrences belonged to (newest
    /// first), so an operator can pick which trace to open. `serde(default)`
    /// tolerates payloads from agents predating traces.
    #[serde(default)]
    pub traces: Vec<crate::TraceSummary>,
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

/// Payload for updating an exception group's triage state. Triage is scoped to
/// the group's source — the same fingerprint on two applications is two
/// independent failures.
///
/// Resolution and suppression are independent axes; a field left `None` is left
/// unchanged, so a control can toggle one axis without disturbing the other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageInput {
    pub project_id: String,
    /// Set the resolution axis: `Some(true)` resolves (anchored at now),
    /// `Some(false)` reopens, `None` leaves it unchanged.
    #[serde(default)]
    pub resolved: Option<bool>,
    /// Set the suppression axis: `Some(true)` mutes, `Some(false)` unmutes,
    /// `None` leaves it unchanged.
    #[serde(default)]
    pub muted: Option<bool>,
    #[serde(default)]
    pub note: Option<String>,
    /// The source URI the triaged group was seen on.
    pub source: String,
}

#[cfg(test)]
mod tests {
    use super::summary_line;

    #[test]
    fn single_line_message_is_returned_trimmed() {
        assert_eq!(summary_line("  x is undefined  "), "x is undefined");
    }

    #[test]
    fn multiline_message_keeps_only_the_first_line() {
        let message = "Failed to parse configuration file.\n\nCaused by:\n - filter at line 1";
        assert_eq!(summary_line(message), "Failed to parse configuration file.");
    }

    #[test]
    fn leading_blank_lines_are_skipped() {
        assert_eq!(summary_line("\n\n   \nfirst real line\nsecond"), "first real line");
    }

    #[test]
    fn empty_message_yields_empty_summary() {
        assert_eq!(summary_line("   \n  \n"), "");
    }

    #[test]
    fn status_derives_from_axes_with_mute_precedence() {
        use super::{ExceptionGroup, ExceptionStatus};
        let group = |resolved: bool, muted: bool| ExceptionGroup {
            group_id: "g".into(),
            exc_type: "T".into(),
            sample_message: "m".into(),
            count: 1,
            first_seen_ms: 0,
            last_seen_ms: 0,
            status: ExceptionStatus::Unresolved,
            resolved,
            muted,
            note: None,
            trend: Vec::new(),
        };
        assert_eq!(
            ExceptionStatus::from(&group(false, false)),
            ExceptionStatus::Unresolved
        );
        assert_eq!(
            ExceptionStatus::from(&group(true, false)),
            ExceptionStatus::Resolved
        );
        assert_eq!(
            ExceptionStatus::from(&group(false, true)),
            ExceptionStatus::Ignored
        );
        // Suppression wins even when the group is also resolved.
        assert_eq!(
            ExceptionStatus::from(&group(true, true)),
            ExceptionStatus::Ignored
        );
    }
}
