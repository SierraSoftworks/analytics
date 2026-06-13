use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What kind of thing a source is. Both flow through the same event pipeline; the
/// distinction drives which metrics make sense in the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    #[default]
    Website,
    Application,
}

/// A source is a hostname that reports events. Sources are created automatically on
/// first sight (unassigned) and can later be grouped into a project via the admin UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub hostname: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub kind: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<DateTime<Utc>>,
}

/// Payload for assigning/updating a source's project, kind, and display name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SourceInput {
    /// `Some(None)` is not expressible in JSON; send an empty string to unassign.
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub kind: Option<SourceKind>,
    #[serde(default)]
    pub display_name: Option<String>,
}
