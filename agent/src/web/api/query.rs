//! Shared query-parameter parsing for the stats endpoints.

use serde::Deserialize;

const DAY_MS: i64 = 86_400_000;

#[derive(Deserialize)]
pub struct StatsQuery {
    /// Range start (epoch millis); defaults to 7 days before `to`.
    pub from: Option<i64>,
    /// Range end (epoch millis); defaults to now.
    pub to: Option<i64>,
    /// Time-series bucket: `minute` | `hour` | `6h` | `day` (default) | `week`.
    pub interval: Option<String>,
    /// Comma-separated subset of source URIs to filter to.
    pub sources: Option<String>,
}

/// Resolve `(from_ms, to_ms, bucket_ms)` from the query, applying defaults.
pub fn resolve_range(query: &StatsQuery) -> (i64, i64, i64) {
    let now = chrono::Utc::now().timestamp_millis();
    let to = query.to.unwrap_or(now);
    let from = query.from.unwrap_or(to - 7 * DAY_MS);
    let bucket = match query.interval.as_deref() {
        Some("minute") => 60_000,
        Some("hour") => 3_600_000,
        Some("6h") => 6 * 3_600_000,
        Some("week") => 7 * DAY_MS,
        _ => DAY_MS,
    };
    (from, to, bucket)
}

/// The optional source-URI subset to filter to.
pub fn subset(query: &StatsQuery) -> Option<Vec<String>> {
    query.sources.as_ref().map(|s| {
        s.split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect()
    })
}
