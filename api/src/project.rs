use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A project groups multiple sources (hostnames/pixels) so their statistics can be
/// viewed in aggregate, e.g. a marketing site plus its docs and backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub created_at: DateTime<Utc>,
}

/// Payload for creating or updating a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInput {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
}
