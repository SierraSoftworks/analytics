use analytics_api::ExceptionStatus;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Admin-set triage state for an exception group, keyed by `(project_id, group_id)`.
/// This is the only mutable exception state; occurrences are append-only events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionTriage {
    pub status: ExceptionStatus,
    pub note: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<String>,
}
