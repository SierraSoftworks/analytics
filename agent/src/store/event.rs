use serde::{Deserialize, Serialize};

/// The kind of an ingested event. All kinds flow through the same append-only
/// pipeline; columnar storage keeps the kind so queries can filter by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    #[default]
    PageLoad,
    PageUnload,
    Custom,
    Pixel,
    Exception,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::PageLoad => "page_load",
            EventKind::PageUnload => "page_unload",
            EventKind::Custom => "custom",
            EventKind::Pixel => "pixel",
            EventKind::Exception => "exception",
        }
    }
}

/// A fully enriched, anonymized event as persisted to redb and Parquet.
///
/// Attribution is a single canonical `source` URI (`https://<hostname>`,
/// `app://<appname>`, `pixel://<id>`, extensible to future schemes); the owning
/// project is resolved at query time from the sources/pixels maps, never stored
/// here. No raw IP, User-Agent, Accept-Language, or cookies are ever retained —
/// only derived classes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredEvent {
    /// Client-reported event time (epoch millis).
    pub created_ms: i64,
    /// Server receive time (epoch millis); also the basis for the storage key.
    pub received_ms: i64,
    /// Monotonic per-event sequence (the low half of the redb key), unique forever.
    /// Carried into Parquet so the hot∪cold union can be de-duplicated if a crash
    /// causes a compaction window to be archived twice. Assigned by the store on
    /// append; ignore the value before then.
    #[serde(default)]
    pub seq: u64,
    /// Per-page-load beacon id linking the events of a single page view.
    pub bid: String,
    pub kind: EventKind,

    /// Canonical source URI this event is attributed to.
    pub source: String,

    pub pathname: Option<String>,
    pub is_unique_user: bool,
    pub is_unique_page: bool,

    pub referrer_host: Option<String>,
    pub referrer_group: Option<String>,
    pub country: Option<String>,
    pub language: Option<String>,
    pub ua_browser: Option<String>,
    pub ua_os: Option<String>,
    pub ua_device: Option<String>,
    pub utm_source: Option<String>,
    pub utm_medium: Option<String>,
    pub utm_campaign: Option<String>,
    pub duration_ms: Option<i64>,

    /// Event name for `Custom`/`Pixel` events.
    pub event_name: Option<String>,
    /// Arbitrary key/value metadata (JSON object) for custom/pixel/exception events.
    pub metadata_json: Option<String>,

    /// The reporting application's version, for attributing failures to a
    /// specific release (exception events; the application itself is the
    /// event's `source`). `serde(default)` keeps pre-attribution redb rows
    /// readable.
    #[serde(default)]
    pub app_version: Option<String>,

    // Exception-only columns.
    pub exc_type: Option<String>,
    pub exc_message: Option<String>,
    pub exc_stack: Option<String>,
    pub exc_group: Option<String>,
    pub exc_handled: Option<bool>,
}
