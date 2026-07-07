use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What the tracking beacon reports. Short JSON keys keep the beacon payload small.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackEvent {
    /// Per-page-load beacon id, linking the events of a single page view.
    #[serde(rename = "b")]
    pub beacon: String,
    /// Per-visit session id, linking the page views of one continuous visit.
    /// Tab-scoped: the tracker keeps it in `sessionStorage`, so it survives
    /// full page navigations on traditional sites but never outlives the tab
    /// (and is never a cookie). The same key carries it on exception reports.
    #[serde(rename = "i", default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(rename = "e", default)]
    pub kind: BeaconKind,
    /// Full page URL (hostname + path + query are derived server-side).
    #[serde(rename = "u")]
    pub url: String,
    #[serde(rename = "r", default, skip_serializing_if = "Option::is_none")]
    pub referrer: Option<String>,
    /// First visit to this site today (from the `/track/ping` oracle).
    #[serde(rename = "q", default)]
    pub unique_visit: bool,
    /// First view of this page today (approximate in v1).
    #[serde(rename = "p", default)]
    pub unique_page: bool,
    /// IANA timezone, e.g. "America/New_York" (mapped to a country server-side).
    #[serde(rename = "t", default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Time on page in milliseconds, sent on unload.
    #[serde(rename = "m", default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// Custom event name (when `kind` is `custom`).
    #[serde(rename = "n", default, skip_serializing_if = "Option::is_none")]
    pub event_name: Option<String>,
    /// Custom event metadata.
    #[serde(rename = "d", default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BeaconKind {
    #[default]
    Load,
    Unload,
    Custom,
}
