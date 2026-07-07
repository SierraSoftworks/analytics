use serde::{Deserialize, Serialize};

/// The kind of an event on a session trace timeline (the stored event kinds,
/// minus `pixel` — pixels carry no session and so never appear on a trace).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceEventKind {
    PageLoad,
    PageUnload,
    Custom,
    Exception,
}

/// A session summarized for the dashboard's recent-traces list: enough context
/// to recognise the visit (where it landed, from where, on what client) without
/// shipping the whole timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceSummary {
    pub session_id: String,
    /// First and last event instants (epoch millis) within the queried window.
    pub started_ms: i64,
    pub last_ms: i64,
    /// Canonical source URI the session reported to.
    pub source: String,
    /// The first page viewed in the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// The client application (a browser or an application client) + version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ua_browser: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ua_version: Option<String>,
    /// The release the reporting application claimed for itself (exception
    /// reports carry it), pinning the trace to a specific version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    pub pageviews: i64,
    pub events: i64,
    pub exceptions: i64,
}

/// One event on a session's timeline. Which optional fields are present
/// depends on [`TraceEvent::kind`]: page views carry a path (and, via their
/// unload, a duration), custom events a name/metadata, exceptions the error
/// context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEvent {
    pub received_ms: i64,
    pub kind: TraceEventKind,
    /// Per-page-view beacon id, pairing a `page_load` with its `page_unload`.
    #[serde(default)]
    pub bid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pathname: Option<String>,
    /// Time on page in milliseconds (unload events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// Custom event name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_name: Option<String>,
    /// Custom/exception metadata as a raw JSON object string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exc_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exc_message: Option<String>,
    /// The exception's grouping fingerprint, linking back to its group page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exc_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exc_handled: Option<bool>,
}

/// A whole session in forensic detail: the visit's context plus its ordered
/// timeline of page views, custom events, and exceptions
/// (`GET /api/v1/traces/{session_id}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTrace {
    pub session_id: String,
    pub started_ms: i64,
    pub ended_ms: i64,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ua_browser: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ua_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ua_os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    /// Oldest first.
    pub events: Vec<TraceEvent>,
}
