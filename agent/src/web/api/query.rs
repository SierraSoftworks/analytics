//! Shared query-parameter helpers for the stats and exceptions endpoints.

const DAY_MS: i64 = 86_400_000;

/// The bucket-size ladder, finest first. Ranges are clamped to the coarsest rung
/// when even `week` would overflow [`MAX_BUCKETS`].
const BUCKETS_MS: &[(&str, i64)] = &[
    ("minute", 60_000),
    ("hour", 3_600_000),
    ("6h", 6 * 3_600_000),
    ("day", DAY_MS),
    ("week", 7 * DAY_MS),
];

/// The most buckets a time series may span. A requested interval that would
/// produce more (e.g. a stale `interval=minute` URL over a 90-day range) is
/// coarsened up the ladder rather than silently degrading into a sparse,
/// gap-filled series.
const MAX_BUCKETS: i64 = 2_000;

/// The latest instant a query may reference (2100-01-01). Clamping client
/// timestamps into `[0, MAX_INSTANT_MS]` keeps the window arithmetic (spans,
/// previous-window shifts) far from `i64` overflow for any crafted input.
pub const MAX_INSTANT_MS: i64 = 4_102_444_800_000;

/// Resolve `(from_ms, to_ms, bucket_ms)` from optional query parameters. The
/// range is half-open `[from, to)`; `to` defaults to now and `from` to 7 days
/// earlier. Inputs are clamped to sane instants with `from < to` guaranteed,
/// and the bucket (default `day`) is clamped so the series stays under
/// [`MAX_BUCKETS`] points.
pub fn resolve_range(
    from: Option<i64>,
    to: Option<i64>,
    interval: Option<&str>,
) -> (i64, i64, i64) {
    let now = chrono::Utc::now().timestamp_millis();
    let to = to.unwrap_or(now).clamp(1, MAX_INSTANT_MS);
    let from = from
        .unwrap_or(to.saturating_sub(7 * DAY_MS))
        .clamp(0, to - 1);
    let span = (to - from).max(1);

    let requested = interval
        .and_then(|name| BUCKETS_MS.iter().find(|(n, _)| *n == name))
        .map(|(_, ms)| *ms)
        .unwrap_or(DAY_MS);

    let bucket = BUCKETS_MS
        .iter()
        .map(|(_, ms)| *ms)
        .find(|ms| *ms >= requested && span / ms <= MAX_BUCKETS)
        // A span too wide even for weekly buckets (decades) gets a bucket sized
        // to stay under the cap rather than a sparse, gap-filled series.
        .unwrap_or_else(|| (span / MAX_BUCKETS + 1).max(7 * DAY_MS));

    (from, to, bucket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_a_week_of_daily_buckets() {
        let (from, to, bucket) = resolve_range(None, None, None);
        assert_eq!(to - from, 7 * DAY_MS);
        assert_eq!(bucket, DAY_MS);
    }

    #[test]
    fn honours_a_reasonable_interval() {
        let (_, _, bucket) = resolve_range(Some(0), Some(DAY_MS), Some("hour"));
        assert_eq!(bucket, 3_600_000);
    }

    #[test]
    fn coarsens_an_interval_that_would_overflow_the_series() {
        // 90 days of minute buckets would be 129,600 points; hourly is still over
        // the cap (2,160), so the ladder settles on 6-hour buckets (360).
        let (_, _, bucket) = resolve_range(Some(0), Some(90 * DAY_MS), Some("minute"));
        assert_eq!(bucket, 6 * 3_600_000);
    }

    #[test]
    fn clamp_keeps_bucket_count_under_the_cap() {
        for days in [1, 7, 30, 90, 365, 3650] {
            let span = days * DAY_MS;
            let (_, _, bucket) = resolve_range(Some(0), Some(span), Some("minute"));
            assert!(
                span / bucket <= MAX_BUCKETS,
                "{days}d span yields too many buckets"
            );
        }
    }

    #[test]
    fn hostile_timestamps_are_clamped_without_overflow() {
        // i64::MIN / MAX and inverted ranges must resolve to a sane window
        // rather than overflowing the span arithmetic.
        for (from, to) in [
            (Some(i64::MIN), None),
            (None, Some(i64::MAX)),
            (Some(i64::MIN), Some(i64::MAX)),
            (Some(10_000), Some(5_000)),
            (Some(5_000), Some(5_000)),
        ] {
            let (from, to, bucket) = resolve_range(from, to, Some("minute"));
            assert!(from < to, "resolved from must precede to");
            assert!((0..=MAX_INSTANT_MS).contains(&from));
            assert!((1..=MAX_INSTANT_MS).contains(&to));
            assert!((to - from) / bucket <= MAX_BUCKETS);
        }
    }
}
