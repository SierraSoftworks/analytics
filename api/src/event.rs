use serde::{Deserialize, Serialize};

/// How a custom/pixel event's occurrences distribute across key dimensions
/// (empty keys mean the dimension was absent on those occurrences). Custom
/// events are enriched like page views, so the full dimension set applies;
/// pixel hits carry almost none of it and fold under the sentinels.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EventBreakdowns {
    pub sources: Vec<crate::CountRow>,
    /// The pages the event fired on.
    pub pages: Vec<crate::CountRow>,
    pub browsers: Vec<crate::CountRow>,
    pub operating_systems: Vec<crate::CountRow>,
    pub devices: Vec<crate::CountRow>,
    /// ISO country codes (the UI maps them to names/flags).
    pub countries: Vec<crate::CountRow>,
    pub languages: Vec<crate::CountRow>,
}

/// A distinct example within an event's occurrences: those sharing the same
/// reporter-supplied metadata, collapsed into one representative with a count.
/// The detail page scrubs through these rather than paging a flat list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventVariant {
    /// Reporter-supplied metadata as a raw JSON object string; `None` groups
    /// the metadata-less occurrences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
    /// How many occurrences share this exact metadata.
    pub count: i64,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    /// Client context from the most recent occurrence of this variant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ua_browser: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ua_os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The page the most recent occurrence fired on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pathname: Option<String>,
    /// The session the most recent session-linked occurrence belonged to,
    /// linking the exemplar to its session trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// One named custom/pixel event in forensic detail, for
/// `GET /api/v1/events?name=…`: the aggregate (with an occurrence trend on the
/// same bucket grid as exception trends), dimension distributions, distinct
/// metadata exemplars, and the sessions it occurred in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventDetail {
    pub name: String,
    pub count: i64,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    /// Occurrence counts over the query range, split into
    /// [`crate::TREND_BUCKETS`] equal buckets (oldest first).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trend: Vec<i64>,
    #[serde(default)]
    pub breakdowns: EventBreakdowns,
    pub variants: Vec<EventVariant>,
    /// The most recent sessions the event occurred in (newest first).
    #[serde(default)]
    pub traces: Vec<crate::TraceSummary>,
}
