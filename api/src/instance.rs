use serde::{Deserialize, Serialize};

/// Authenticated instance/runtime information shown on the Settings page. Unlike
/// the public health endpoint (which deliberately reveals nothing), this exposes
/// the running version and operational posture, so it is only reachable from
/// behind the admin ACL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    /// The running software version (a release tag, or `0.0.0-dev` in debug builds).
    pub version: String,
    /// How long cold Parquet partitions are retained, in days.
    pub retention_days: u64,
    /// How long events stay in the hot store before compaction, in hours.
    pub hot_window_hours: u64,
    /// Whether `DNT` / `Sec-GPC` signals are honoured by dropping the beacon.
    pub honor_dnt: bool,
    /// Whether per-IP rate limiting is enabled.
    pub rate_limiting: bool,
    /// Sustained per-IP requests/minute allowed on the public tracking endpoints.
    pub tracking_per_minute: u32,
    /// Sustained per-IP requests/minute allowed for unauthenticated API hits.
    pub unauthenticated_per_minute: u32,
    /// Ceiling on auto-registered (unassigned) sources.
    pub max_auto_sources: u64,
}
