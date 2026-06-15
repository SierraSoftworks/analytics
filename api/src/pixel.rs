use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A pre-generated tracking GIF, bound to a project, with metadata recorded on every
/// hit. Requesting an unknown pixel id is rejected, so there is no open pixel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pixel {
    pub id: String,
    pub project_id: String,
    pub name: String,
    /// The event name recorded for each hit (defaults to "pixel").
    pub event_name: String,
    /// Static metadata attached to every event produced by this pixel.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_hit: Option<DateTime<Utc>>,
}

/// Payload for creating a pixel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelInput {
    pub name: String,
    #[serde(default)]
    pub event_name: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}
