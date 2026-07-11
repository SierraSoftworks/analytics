use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Admin-set triage state for an exception group, keyed by `(project_id, group_id)`.
/// This is the only mutable exception state; occurrences are append-only events.
///
/// Resolution and suppression are two independent axes:
/// - `resolved_at` is the sole source of truth for resolution. A group counts as
///   resolved only while its most recent occurrence predates it — an occurrence
///   *after* `resolved_at` is a regression, so the group surfaces as unresolved
///   again automatically (see [`ExceptionTriage::is_resolved`]).
/// - `muted_at` suppresses a group independently of whether it is resolved; a
///   recurrence never lifts it (muting is a deliberate "stop showing me this").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionTriage {
    #[serde(default)]
    pub resolved_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub muted_at: Option<DateTime<Utc>>,
    pub note: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<String>,
}

impl ExceptionTriage {
    /// Whether the group is currently resolved: it was resolved at some point and
    /// has not been seen again since. `last_seen_ms` is the group's most recent
    /// occurrence within the viewed window; a newer occurrence is a regression and
    /// reopens the group without any write.
    pub fn is_resolved(&self, last_seen_ms: i64) -> bool {
        self.resolved_at
            .is_some_and(|at| last_seen_ms <= at.timestamp_millis())
    }

    /// Whether the group is suppressed. Independent of resolution and unaffected
    /// by recurrence.
    pub fn is_muted(&self) -> bool {
        self.muted_at.is_some()
    }
}
