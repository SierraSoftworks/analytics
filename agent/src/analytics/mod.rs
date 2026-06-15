//! The polars query layer. Statistics are computed over the union of the redb hot
//! store and the cold Parquet partitions, filtered to a project's source URIs and a
//! time range. Queries are CPU-bound and synchronous, so handlers run them via
//! `web::block`.

use std::path::Path;

use analytics_api::{
    ExceptionGroup, ExceptionOccurrence, ExceptionStatus, KeyCount, MetricSummary, Overview,
    ProjectSummary, SourceSummary, Stats, TimeSeriesPoint, pixel_source,
};
use chrono::{Datelike, TimeZone, Utc};
use polars::prelude::*;
use tracing_batteries::prelude::warn;

use crate::errors::{Result, ResultExt};
use crate::store::Store;

const ADVICE: &[&str] = &["This is an internal analytics error; please report it with the logs."];

const BREAKDOWN_LIMIT: u32 = 25;
/// `[100ms, 5s]` is treated as a bounce (per the medama methodology).
const BOUNCE_MIN_MS: i64 = 100;
const BOUNCE_MAX_MS: i64 = 5_000;
const MIN_BOUNCE_SAMPLES: i64 = 5;

/// Full statistics for a set of source URIs over `[from_ms, to_ms]`.
pub fn stats_for_sources(
    store: &Store,
    parquet_dir: &str,
    sources: &[String],
    from_ms: i64,
    to_ms: i64,
    bucket_ms: i64,
) -> Result<Stats> {
    let base = combined(store, parquet_dir, from_ms, to_ms)?.filter(source_filter(sources));
    let pageloads = base.clone().filter(col("kind").eq(lit("page_load")));

    Ok(Stats {
        summary: summary(base.clone())?,
        timeseries: timeseries(base, from_ms, to_ms, bucket_ms)?,
        pages: breakdown(pageloads.clone(), "pathname")?,
        referrers: breakdown(pageloads.clone(), "referrer_host")?,
        browsers: breakdown(pageloads.clone(), "ua_browser")?,
        operating_systems: breakdown(pageloads.clone(), "ua_os")?,
        devices: breakdown(pageloads.clone(), "ua_device")?,
        countries: breakdown(pageloads.clone(), "country")?,
        languages: breakdown(pageloads.clone(), "language")?,
        sources: breakdown(pageloads, "source")?,
    })
}

/// The global overview across all projects, with per-project and unassigned totals.
pub fn overview(
    store: &Store,
    parquet_dir: &str,
    from_ms: i64,
    to_ms: i64,
    bucket_ms: i64,
) -> Result<Overview> {
    let projects = store.list_projects()?;
    let sources = store.list_sources()?;

    let base = combined(store, parquet_dir, from_ms, to_ms)?;
    let summary = summary(base.clone())?;
    let timeseries = timeseries(base.clone(), from_ms, to_ms, bucket_ms)?;
    let per_source = per_source_totals(base)?;

    // Build a source-URI -> project map from assigned sources and pixels.
    let pixels = store.list_pixels()?;
    let mut uri_project: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for source in &sources {
        if let Some(project_id) = &source.project_id {
            uri_project.insert(source.uri.clone(), project_id.clone());
        }
    }
    for pixel in &pixels {
        uri_project.insert(pixel_source(&pixel.id), pixel.project_id.clone());
    }

    // Aggregate per-source totals up to projects, and collect unassigned sources
    // (keeping both their visitor and view totals).
    let mut project_totals: std::collections::HashMap<String, (i64, i64)> =
        std::collections::HashMap::new();
    let mut unassigned: Vec<SourceSummary> = Vec::new();
    for (uri, visitors, pageviews) in &per_source {
        match uri_project.get(uri) {
            Some(project_id) => {
                let entry = project_totals.entry(project_id.clone()).or_insert((0, 0));
                entry.0 += visitors;
                entry.1 += pageviews;
            }
            None => unassigned.push(SourceSummary {
                uri: uri.clone(),
                visitors: *visitors,
                pageviews: *pageviews,
            }),
        }
    }

    let mut project_summaries: Vec<ProjectSummary> = projects
        .into_iter()
        .map(|project| {
            let (visitors, pageviews) = project_totals.get(&project.id).copied().unwrap_or((0, 0));
            ProjectSummary {
                project,
                visitors,
                pageviews,
            }
        })
        .collect();
    project_summaries.sort_by_key(|p| std::cmp::Reverse(p.pageviews));
    unassigned.sort_by_key(|u| std::cmp::Reverse(u.pageviews));

    Ok(Overview {
        summary,
        timeseries,
        projects: project_summaries,
        unassigned,
    })
}

/// The source URIs belonging to a project: its assigned sources plus its pixels
/// (as `pixel://<id>` URIs).
pub fn project_source_uris(store: &Store, project_id: &str) -> Result<Vec<String>> {
    let mut uris: Vec<String> = store
        .list_sources()?
        .into_iter()
        .filter(|s| s.project_id.as_deref() == Some(project_id))
        .map(|s| s.uri)
        .collect();
    for pixel in store.list_pixels()? {
        if pixel.project_id == project_id {
            uris.push(pixel_source(&pixel.id));
        }
    }
    Ok(uris)
}

/// Aggregated exception groups for a set of sources over a time range. The triage
/// status is filled in by the caller (which knows the project).
pub fn exception_groups(
    store: &Store,
    parquet_dir: &str,
    sources: &[String],
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<ExceptionGroup>> {
    let df = combined(store, parquet_dir, from_ms, to_ms)?
        .filter(source_filter(sources))
        .filter(col("kind").eq(lit("exception")))
        .filter(col("exc_group").is_not_null())
        .group_by([col("exc_group")])
        .agg([
            len().cast(DataType::Int64).alias("count"),
            col("received_ms")
                .min()
                .cast(DataType::Int64)
                .alias("first_seen"),
            col("received_ms")
                .max()
                .cast(DataType::Int64)
                .alias("last_seen"),
            col("exc_type").first().alias("exc_type"),
            col("exc_message").first().alias("sample_message"),
        ])
        .sort(
            ["last_seen"],
            SortMultipleOptions::default().with_order_descending(true),
        )
        .limit(200)
        .collect()
        .or_system_err(ADVICE)?;

    let group_id = df
        .column("exc_group")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let count = df
        .column("count")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let first = df
        .column("first_seen")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let last = df
        .column("last_seen")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let exc_type = df
        .column("exc_type")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let message = df
        .column("sample_message")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;

    Ok((0..df.height())
        .filter_map(|i| {
            group_id.get(i).map(|gid| ExceptionGroup {
                group_id: gid.to_string(),
                exc_type: exc_type.get(i).unwrap_or("").to_string(),
                sample_message: message.get(i).unwrap_or("").to_string(),
                count: count.get(i).unwrap_or(0),
                first_seen_ms: first.get(i).unwrap_or(0),
                last_seen_ms: last.get(i).unwrap_or(0),
                status: ExceptionStatus::Unresolved,
                note: None,
            })
        })
        .collect())
}

/// Like [`exception_groups`] but across **every** source, grouped by
/// `(fingerprint, source)`. A fingerprint is computed from the error alone, so the
/// same `exc_group` legitimately occurs on multiple sources/projects; keeping the
/// source in the key keeps those occurrences separate. The caller folds per-source
/// rows up to per-project rows for the global Exceptions inbox.
pub fn global_exception_groups(
    store: &Store,
    parquet_dir: &str,
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<(ExceptionGroup, String)>> {
    let df = combined(store, parquet_dir, from_ms, to_ms)?
        .filter(col("kind").eq(lit("exception")))
        .filter(col("exc_group").is_not_null())
        .group_by([col("exc_group"), col("source")])
        .agg([
            len().cast(DataType::Int64).alias("count"),
            col("received_ms")
                .min()
                .cast(DataType::Int64)
                .alias("first_seen"),
            col("received_ms")
                .max()
                .cast(DataType::Int64)
                .alias("last_seen"),
            col("exc_type").first().alias("exc_type"),
            col("exc_message").first().alias("sample_message"),
        ])
        .sort(
            ["last_seen"],
            SortMultipleOptions::default().with_order_descending(true),
        )
        .limit(500)
        .collect()
        .or_system_err(ADVICE)?;

    let group_id = df
        .column("exc_group")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let count = df
        .column("count")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let first = df
        .column("first_seen")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let last = df
        .column("last_seen")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let exc_type = df
        .column("exc_type")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let message = df
        .column("sample_message")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let source = df
        .column("source")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;

    Ok((0..df.height())
        .filter_map(|i| {
            group_id.get(i).map(|gid| {
                (
                    ExceptionGroup {
                        group_id: gid.to_string(),
                        exc_type: exc_type.get(i).unwrap_or("").to_string(),
                        sample_message: message.get(i).unwrap_or("").to_string(),
                        count: count.get(i).unwrap_or(0),
                        first_seen_ms: first.get(i).unwrap_or(0),
                        last_seen_ms: last.get(i).unwrap_or(0),
                        status: ExceptionStatus::Unresolved,
                        note: None,
                    },
                    source.get(i).unwrap_or("").to_string(),
                )
            })
        })
        .collect())
}

/// A single exception group with its most-recent occurrences, derived from **one**
/// scan filtered to that group. Looked up by id directly (no top-N cap), so a linked
/// or bookmarked group opens regardless of how many fingerprints a project has.
/// Returns `None` if the group has no occurrences in `[from_ms, to_ms]`.
pub fn exception_detail(
    store: &Store,
    parquet_dir: &str,
    sources: &[String],
    group_id: &str,
    from_ms: i64,
    to_ms: i64,
    limit: usize,
) -> Result<Option<(ExceptionGroup, Vec<ExceptionOccurrence>)>> {
    let df = combined(store, parquet_dir, from_ms, to_ms)?
        .filter(source_filter(sources))
        .filter(col("kind").eq(lit("exception")))
        .filter(col("exc_group").eq(lit(group_id.to_string())))
        .select([
            col("exc_type"),
            col("exc_message"),
            col("exc_stack"),
            col("exc_handled"),
            col("received_ms")
                .cast(DataType::Int64)
                .alias("received_ms"),
            col("ua_browser"),
            col("ua_os"),
        ])
        .sort(
            ["received_ms"],
            SortMultipleOptions::default().with_order_descending(true),
        )
        .collect()
        .or_system_err(ADVICE)?;

    let height = df.height();
    if height == 0 {
        return Ok(None);
    }

    let exc_type = df
        .column("exc_type")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let message = df
        .column("exc_message")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let stack = df
        .column("exc_stack")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let handled = df
        .column("exc_handled")
        .or_system_err(ADVICE)?
        .bool()
        .or_system_err(ADVICE)?;
    let received = df
        .column("received_ms")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let browser = df
        .column("ua_browser")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let os = df
        .column("ua_os")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;

    // Rows are newest-first: index 0 is the most recent occurrence, the last index the
    // oldest. The aggregate spans every row; the returned occurrences keep `limit`.
    let group = ExceptionGroup {
        group_id: group_id.to_string(),
        exc_type: exc_type.get(0).unwrap_or("").to_string(),
        sample_message: message.get(0).unwrap_or("").to_string(),
        count: height as i64,
        first_seen_ms: received.get(height - 1).unwrap_or(0),
        last_seen_ms: received.get(0).unwrap_or(0),
        status: ExceptionStatus::Unresolved,
        note: None,
    };

    let occurrences = (0..height.min(limit))
        .map(|i| ExceptionOccurrence {
            exc_type: exc_type.get(i).unwrap_or("").to_string(),
            message: message.get(i).unwrap_or("").to_string(),
            stack: stack.get(i).map(str::to_string),
            handled: handled.get(i).unwrap_or(false),
            received_ms: received.get(i).unwrap_or(0),
            ua_browser: browser.get(i).map(str::to_string),
            ua_os: os.get(i).map(str::to_string),
        })
        .collect();

    Ok(Some((group, occurrences)))
}

// ----------------------------------------------------------------- internals

/// The time-filtered union of the cold Parquet partitions and the redb hot store.
fn combined(store: &Store, parquet_dir: &str, from_ms: i64, to_ms: i64) -> Result<LazyFrame> {
    let mut frames: Vec<LazyFrame> = Vec::new();
    for file in parquet_files_in_range(Path::new(parquet_dir), from_ms, to_ms) {
        let path = file.to_string_lossy();
        match LazyFrame::scan_parquet(PlRefPath::from(path.as_ref()), ScanArgsParquet::default()) {
            Ok(lf) => frames.push(lf),
            // A corrupt/unreadable partition must surface in the logs, not silently
            // drop events from every query that touches its date range.
            Err(err) => warn!("skipping unreadable parquet partition {path}: {err}"),
        }
    }
    frames.push(store.hot_dataframe()?.lazy());

    let combined = if frames.len() == 1 {
        frames.pop().expect("one frame")
    } else {
        concat(
            frames,
            UnionArgs {
                to_supertypes: true,
                ..Default::default()
            },
        )
        .or_system_err(ADVICE)?
    };

    // De-duplicate the union: if a crash leaves a compaction window present in both
    // Parquet and redb, the same events appear twice. Every row carries the globally
    // unique per-event `seq`, so two *distinct* events can never be all-columns-equal;
    // a full-row unique therefore collapses exactly the crash duplicates and nothing
    // else.
    Ok(combined
        .filter(
            col("received_ms")
                .gt_eq(lit(from_ms))
                .and(col("received_ms").lt_eq(lit(to_ms))),
        )
        .unique(None, UniqueKeepStrategy::Any))
}

/// An OR-chain matching any of the given source URIs (empty set matches nothing).
fn source_filter(sources: &[String]) -> Expr {
    let mut expr = lit(false);
    for source in sources {
        expr = expr.or(col("source").eq(lit(source.clone())));
    }
    expr
}

fn summary(base: LazyFrame) -> Result<MetricSummary> {
    let df = base
        .select([
            col("kind")
                .eq(lit("page_load"))
                .sum()
                .cast(DataType::Int64)
                .alias("pageviews"),
            col("kind")
                .eq(lit("page_load"))
                .and(col("is_unique_user"))
                .sum()
                .cast(DataType::Int64)
                .alias("visitors"),
            col("duration_ms")
                .is_not_null()
                .sum()
                .cast(DataType::Int64)
                .alias("samples"),
            col("duration_ms")
                .gt_eq(lit(BOUNCE_MIN_MS))
                .and(col("duration_ms").lt_eq(lit(BOUNCE_MAX_MS)))
                .sum()
                .cast(DataType::Int64)
                .alias("bounces"),
            col("duration_ms").median().alias("median_ms"),
        ])
        .collect()
        .or_system_err(ADVICE)?;

    let samples = scalar_i64(&df, "samples");
    let bounces = scalar_i64(&df, "bounces");
    let median = df
        .column("median_ms")
        .ok()
        .and_then(|c| c.f64().ok())
        .and_then(|a| a.get(0));

    Ok(MetricSummary {
        visitors: scalar_i64(&df, "visitors"),
        pageviews: scalar_i64(&df, "pageviews"),
        bounce_rate: (samples >= MIN_BOUNCE_SAMPLES).then(|| bounces as f64 / samples as f64),
        median_duration_ms: median.map(|m| m.round() as i64),
    })
}

/// A continuous time series over `[from_ms, to_ms]` at `bucket_ms` resolution.
/// Buckets with no events are emitted as zeros so the chart shows a gap-free line
/// across the whole window instead of collapsing absent periods.
fn timeseries(
    base: LazyFrame,
    from_ms: i64,
    to_ms: i64,
    bucket_ms: i64,
) -> Result<Vec<TimeSeriesPoint>> {
    let bucket_ms = bucket_ms.max(1);
    let df = base
        .filter(col("kind").eq(lit("page_load")))
        .with_columns([(col("received_ms") - col("received_ms") % lit(bucket_ms)).alias("bucket")])
        .group_by([col("bucket")])
        .agg([
            len().cast(DataType::Int64).alias("pageviews"),
            col("is_unique_user")
                .sum()
                .cast(DataType::Int64)
                .alias("visitors"),
        ])
        .collect()
        .or_system_err(ADVICE)?;

    let bucket = df
        .column("bucket")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let pageviews = df
        .column("pageviews")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let visitors = df
        .column("visitors")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;

    // Index the populated buckets, then walk every bucket in the window.
    let mut counts: std::collections::HashMap<i64, (i64, i64)> = std::collections::HashMap::new();
    for i in 0..df.height() {
        if let Some(b) = bucket.get(i) {
            counts.insert(
                b,
                (pageviews.get(i).unwrap_or(0), visitors.get(i).unwrap_or(0)),
            );
        }
    }

    let first = from_ms - from_ms.rem_euclid(bucket_ms);
    let last = to_ms - to_ms.rem_euclid(bucket_ms);
    // Guard against a pathological window/bucket combination producing a huge vec.
    let estimated = ((last - first) / bucket_ms).unsigned_abs() as usize + 1;
    if first > last || estimated > 5_000 {
        // Fall back to the populated buckets only (sorted).
        let mut points: Vec<TimeSeriesPoint> = counts
            .into_iter()
            .map(|(b, (p, v))| TimeSeriesPoint {
                timestamp_ms: b,
                pageviews: p,
                visitors: v,
            })
            .collect();
        points.sort_by_key(|p| p.timestamp_ms);
        return Ok(points);
    }

    let mut points = Vec::with_capacity(estimated);
    let mut b = first;
    while b <= last {
        let (pageviews, visitors) = counts.get(&b).copied().unwrap_or((0, 0));
        points.push(TimeSeriesPoint {
            timestamp_ms: b,
            pageviews,
            visitors,
        });
        b += bucket_ms;
    }
    Ok(points)
}

fn breakdown(pageloads: LazyFrame, column: &str) -> Result<Vec<KeyCount>> {
    let df = pageloads
        .filter(col(column).is_not_null())
        .group_by([col(column)])
        .agg([len().cast(DataType::Int64).alias("count")])
        .sort(
            ["count"],
            SortMultipleOptions::default().with_order_descending(true),
        )
        .limit(BREAKDOWN_LIMIT)
        .collect()
        .or_system_err(ADVICE)?;

    let keys = df
        .column(column)
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let counts = df
        .column("count")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;

    Ok((0..df.height())
        .filter_map(|i| {
            keys.get(i).map(|key| KeyCount {
                key: key.to_string(),
                count: counts.get(i).unwrap_or(0),
            })
        })
        .collect())
}

/// Per-source `(uri, visitors, pageviews)` totals. Page loads, pixel hits, and
/// custom events all count toward `pageviews` so pixel-only and application sources
/// still surface on the overview; `visitors` stays page-load specific (only page
/// loads carry the daily-unique flag).
fn per_source_totals(base: LazyFrame) -> Result<Vec<(String, i64, i64)>> {
    let counted = col("kind")
        .eq(lit("page_load"))
        .or(col("kind").eq(lit("pixel")))
        .or(col("kind").eq(lit("custom")));
    let df = base
        .filter(counted)
        .group_by([col("source")])
        .agg([
            len().cast(DataType::Int64).alias("pageviews"),
            col("is_unique_user")
                .sum()
                .cast(DataType::Int64)
                .alias("visitors"),
        ])
        .collect()
        .or_system_err(ADVICE)?;

    let sources = df
        .column("source")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let pageviews = df
        .column("pageviews")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let visitors = df
        .column("visitors")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;

    Ok((0..df.height())
        .filter_map(|i| {
            sources.get(i).map(|s| {
                (
                    s.to_string(),
                    visitors.get(i).unwrap_or(0),
                    pageviews.get(i).unwrap_or(0),
                )
            })
        })
        .collect())
}

fn scalar_i64(df: &DataFrame, name: &str) -> i64 {
    df.column(name)
        .ok()
        .and_then(|c| c.i64().ok())
        .and_then(|a| a.get(0))
        .unwrap_or(0)
}

/// Parquet partition files whose `YYYY/MM/DD` directory overlaps `[from_ms, to_ms]`.
/// Partitions are date-partitioned, so pruning whole day directories keeps a wide
/// query range from scanning the entire archive.
fn parquet_files_in_range(dir: &Path, from_ms: i64, to_ms: i64) -> Vec<std::path::PathBuf> {
    let (from, to) = (day_of(from_ms), day_of(to_ms));
    let mut out = Vec::new();
    for year in numeric_subdirs::<i32>(dir) {
        let year_dir = dir.join(format!("{year:04}"));
        for month in numeric_subdirs::<u32>(&year_dir) {
            let month_dir = year_dir.join(format!("{month:02}"));
            for day in numeric_subdirs::<u32>(&month_dir) {
                let date = (year, month, day);
                if date < from || date > to {
                    continue;
                }
                let day_dir = month_dir.join(format!("{day:02}"));
                let Ok(entries) = std::fs::read_dir(&day_dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|e| e == "parquet") {
                        out.push(path);
                    }
                }
            }
        }
    }
    out
}

/// `(year, month, day)` in UTC for an epoch-millis instant (epoch on overflow).
fn day_of(ms: i64) -> (i32, u32, u32) {
    let dt = Utc
        .timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap());
    (dt.year(), dt.month(), dt.day())
}

/// Numeric subdirectory names (a year/month/day component) directly under `dir`.
fn numeric_subdirs<T: std::str::FromStr>(dir: &Path) -> Vec<T> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse::<T>().ok()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EventKind, StoredEvent};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_redb() -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("analytics-stats-{}-{}.redb", std::process::id(), n))
    }

    fn load(source: &str, received_ms: i64, unique: bool, duration: Option<i64>) -> StoredEvent {
        StoredEvent {
            created_ms: received_ms,
            received_ms,
            bid: "b".into(),
            kind: if duration.is_some() {
                EventKind::PageUnload
            } else {
                EventKind::PageLoad
            },
            source: source.into(),
            pathname: Some("/home".into()),
            is_unique_user: unique,
            ua_browser: Some("Chrome".into()),
            duration_ms: duration,
            ..Default::default()
        }
    }

    #[test]
    fn computes_summary_from_hot_store() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                load("https://a.com", 2_000, false, None),
                load("https://a.com", 3_000, true, None),
                load("https://b.com", 4_000, true, None), // different source, excluded
            ])
            .unwrap();

        // No parquet dir -> hot store only.
        let stats = stats_for_sources(
            &store,
            "/nonexistent-parquet",
            &["https://a.com".to_string()],
            0,
            10_000,
            86_400_000,
        )
        .unwrap();

        assert_eq!(stats.summary.pageviews, 3);
        assert_eq!(stats.summary.visitors, 2); // two unique loads for a.com
        assert_eq!(stats.pages.first().map(|p| p.key.as_str()), Some("/home"));
        assert_eq!(stats.pages.first().map(|p| p.count), Some(3));

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn timeseries_zero_fills_empty_buckets() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let day = 86_400_000i64;
        // Two views on the first day only; the window spans three days.
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                load("https://a.com", 2_000, false, None),
            ])
            .unwrap();

        let stats = stats_for_sources(
            &store,
            "/none",
            &["https://a.com".to_string()],
            0,
            3 * day,
            day,
        )
        .unwrap();

        // Buckets at 0, 1d, 2d, 3d — empty days filled with zeros, not dropped.
        assert_eq!(stats.timeseries.len(), 4);
        assert_eq!(stats.timeseries[0].pageviews, 2);
        assert!(
            stats.timeseries[1..]
                .iter()
                .all(|p| p.pageviews == 0 && p.visitors == 0)
        );

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    fn typed(source: &str, received_ms: i64, kind: EventKind) -> StoredEvent {
        StoredEvent {
            created_ms: received_ms,
            received_ms,
            source: source.into(),
            kind,
            is_unique_user: false,
            ..Default::default()
        }
    }

    fn exc(group: &str, received_ms: i64) -> StoredEvent {
        exc_on("https://a.com", group, received_ms)
    }

    fn exc_on(source: &str, group: &str, received_ms: i64) -> StoredEvent {
        StoredEvent {
            created_ms: received_ms,
            received_ms,
            kind: EventKind::Exception,
            source: source.into(),
            exc_type: Some("TypeError".into()),
            exc_message: Some("boom".into()),
            exc_group: Some(group.into()),
            exc_handled: Some(false),
            ..Default::default()
        }
    }

    #[test]
    fn global_exception_groups_keep_sources_separate() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        // Same fingerprint on two different sources (e.g. a shared-library error).
        store
            .append_events(&[
                exc_on("https://a.com", "g1", 1_000),
                exc_on("https://b.com", "g1", 2_000),
                exc_on("https://a.com", "g1", 3_000),
            ])
            .unwrap();

        let rows = global_exception_groups(&store, "/none", 0, 10_000).unwrap();
        // One row per (fingerprint, source) — not collapsed across sources.
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(g, _)| g.group_id == "g1"));
        let a = rows.iter().find(|(_, s)| s == "https://a.com").unwrap();
        assert_eq!(a.0.count, 2);
        let b = rows.iter().find(|(_, s)| s == "https://b.com").unwrap();
        assert_eq!(b.0.count, 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn union_deduplicates_a_crash_duplicated_window() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                load("https://a.com", 2_000, false, None),
            ])
            .unwrap();

        // Simulate a crash between archive and delete: the same window now lives in
        // both Parquet and the hot store. The archived rows carry the stamped `seq`.
        let archived = store.all_events().unwrap();
        let parquet_dir =
            std::env::temp_dir().join(format!("analytics-dedup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parquet_dir);
        let partition = parquet_dir
            .join("1970")
            .join("01")
            .join("01")
            .join("events-1.parquet");
        crate::store::write_partition(&archived, &partition).unwrap();

        let stats = stats_for_sources(
            &store,
            parquet_dir.to_str().unwrap(),
            &["https://a.com".to_string()],
            0,
            10_000,
            86_400_000,
        )
        .unwrap();

        // Without dedup this would double to 4 pageviews / 2 visitors.
        assert_eq!(stats.summary.pageviews, 2);
        assert_eq!(stats.summary.visitors, 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
        let _ = std::fs::remove_dir_all(&parquet_dir);
    }

    #[test]
    fn exception_group_lookup_ignores_the_recency_cap() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let events: Vec<_> = (1..=205)
            .map(|i| exc(&format!("g{i}"), i * 1_000))
            .collect();
        store.append_events(&events).unwrap();
        let sources = ["https://a.com".to_string()];

        // g1 is the oldest, so it falls outside the top-200-by-recency listing...
        let listed = exception_groups(&store, "/none", &sources, 0, 10_000_000).unwrap();
        assert_eq!(listed.len(), 200);
        assert!(!listed.iter().any(|g| g.group_id == "g1"));

        // ...but a direct lookup still resolves it (group + occurrences in one scan).
        let g1 = exception_detail(&store, "/none", &sources, "g1", 0, 10_000_000, 10).unwrap();
        let (group, occurrences) = g1.expect("g1 resolves");
        assert_eq!(group.group_id, "g1");
        assert_eq!(group.count, 1);
        assert_eq!(occurrences.len(), 1);
        // An unknown group resolves to None.
        assert!(
            exception_detail(&store, "/none", &sources, "nope", 0, 10_000_000, 10)
                .unwrap()
                .is_none()
        );

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn overview_surfaces_pixel_and_custom_sources() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                typed("pixel://p1", 2_000, EventKind::Pixel),
                typed("app://svc", 3_000, EventKind::Custom),
            ])
            .unwrap();

        let overview = overview(&store, "/none", 0, 10_000, 86_400_000).unwrap();
        let uris: Vec<&str> = overview.unassigned.iter().map(|u| u.uri.as_str()).collect();
        assert!(uris.contains(&"https://a.com"));
        assert!(uris.contains(&"pixel://p1")); // previously invisible
        assert!(uris.contains(&"app://svc")); // previously invisible

        // The website keeps its visitor count; pixel/custom add views, not visitors.
        let site = overview
            .unassigned
            .iter()
            .find(|u| u.uri == "https://a.com")
            .unwrap();
        assert_eq!(site.visitors, 1);
        assert_eq!(site.pageviews, 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }
}
