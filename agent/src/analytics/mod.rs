//! The polars query layer. Statistics are computed over the union of the redb hot
//! store and the cold Parquet partitions, filtered to a project's source URIs and a
//! time range. Queries are CPU-bound and synchronous, so handlers run them via
//! `web::block`.

use std::path::Path;

use analytics_api::{
    ExceptionGroup, ExceptionOccurrence, ExceptionStatus, KeyCount, MetricSummary, Overview,
    ProjectSummary, Stats, TimeSeriesPoint, pixel_source,
};
use polars::prelude::*;

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
        timeseries: timeseries(base, bucket_ms)?,
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
    let timeseries = timeseries(base.clone(), bucket_ms)?;
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

    // Aggregate per-source totals up to projects, and collect unassigned sources.
    let mut project_totals: std::collections::HashMap<String, (i64, i64)> =
        std::collections::HashMap::new();
    let mut unassigned: Vec<KeyCount> = Vec::new();
    for (uri, visitors, pageviews) in &per_source {
        match uri_project.get(uri) {
            Some(project_id) => {
                let entry = project_totals.entry(project_id.clone()).or_insert((0, 0));
                entry.0 += visitors;
                entry.1 += pageviews;
            }
            None => unassigned.push(KeyCount {
                key: uri.clone(),
                count: *pageviews,
            }),
        }
    }

    let mut project_summaries: Vec<ProjectSummary> = projects
        .into_iter()
        .map(|project| {
            let (visitors, pageviews) =
                project_totals.get(&project.id).copied().unwrap_or((0, 0));
            ProjectSummary {
                project,
                visitors,
                pageviews,
            }
        })
        .collect();
    project_summaries.sort_by_key(|p| std::cmp::Reverse(p.pageviews));
    unassigned.sort_by_key(|u| std::cmp::Reverse(u.count));

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
            col("received_ms").min().cast(DataType::Int64).alias("first_seen"),
            col("received_ms").max().cast(DataType::Int64).alias("last_seen"),
            col("exc_type").first().alias("exc_type"),
            col("exc_message").first().alias("sample_message"),
        ])
        .sort(["last_seen"], SortMultipleOptions::default().with_order_descending(true))
        .limit(200)
        .collect()
        .or_system_err(ADVICE)?;

    let group_id = df.column("exc_group").or_system_err(ADVICE)?.str().or_system_err(ADVICE)?;
    let count = df.column("count").or_system_err(ADVICE)?.i64().or_system_err(ADVICE)?;
    let first = df.column("first_seen").or_system_err(ADVICE)?.i64().or_system_err(ADVICE)?;
    let last = df.column("last_seen").or_system_err(ADVICE)?.i64().or_system_err(ADVICE)?;
    let exc_type = df.column("exc_type").or_system_err(ADVICE)?.str().or_system_err(ADVICE)?;
    let message = df.column("sample_message").or_system_err(ADVICE)?.str().or_system_err(ADVICE)?;

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

/// Recent occurrences of a single exception group.
pub fn exception_occurrences(
    store: &Store,
    parquet_dir: &str,
    sources: &[String],
    group_id: &str,
    from_ms: i64,
    to_ms: i64,
    limit: u32,
) -> Result<Vec<ExceptionOccurrence>> {
    let df = combined(store, parquet_dir, from_ms, to_ms)?
        .filter(source_filter(sources))
        .filter(col("kind").eq(lit("exception")))
        .filter(col("exc_group").eq(lit(group_id.to_string())))
        .sort(["received_ms"], SortMultipleOptions::default().with_order_descending(true))
        .limit(limit)
        .collect()
        .or_system_err(ADVICE)?;

    let exc_type = df.column("exc_type").or_system_err(ADVICE)?.str().or_system_err(ADVICE)?;
    let message = df.column("exc_message").or_system_err(ADVICE)?.str().or_system_err(ADVICE)?;
    let stack = df.column("exc_stack").or_system_err(ADVICE)?.str().or_system_err(ADVICE)?;
    let handled = df.column("exc_handled").or_system_err(ADVICE)?.bool().or_system_err(ADVICE)?;
    let received = df.column("received_ms").or_system_err(ADVICE)?.i64().or_system_err(ADVICE)?;
    let browser = df.column("ua_browser").or_system_err(ADVICE)?.str().or_system_err(ADVICE)?;
    let os = df.column("ua_os").or_system_err(ADVICE)?.str().or_system_err(ADVICE)?;

    Ok((0..df.height())
        .map(|i| ExceptionOccurrence {
            exc_type: exc_type.get(i).unwrap_or("").to_string(),
            message: message.get(i).unwrap_or("").to_string(),
            stack: stack.get(i).map(str::to_string),
            handled: handled.get(i).unwrap_or(false),
            received_ms: received.get(i).unwrap_or(0),
            ua_browser: browser.get(i).map(str::to_string),
            ua_os: os.get(i).map(str::to_string),
        })
        .collect())
}

// ----------------------------------------------------------------- internals

/// The time-filtered union of the cold Parquet partitions and the redb hot store.
fn combined(store: &Store, parquet_dir: &str, from_ms: i64, to_ms: i64) -> Result<LazyFrame> {
    let mut frames: Vec<LazyFrame> = Vec::new();
    for file in parquet_files(Path::new(parquet_dir)) {
        let path = file.to_string_lossy();
        if let Ok(lf) =
            LazyFrame::scan_parquet(PlRefPath::from(path.as_ref()), ScanArgsParquet::default())
        {
            frames.push(lf);
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

    Ok(combined.filter(
        col("received_ms")
            .gt_eq(lit(from_ms))
            .and(col("received_ms").lt_eq(lit(to_ms))),
    ))
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

fn timeseries(base: LazyFrame, bucket_ms: i64) -> Result<Vec<TimeSeriesPoint>> {
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
        .sort(["bucket"], SortMultipleOptions::default())
        .collect()
        .or_system_err(ADVICE)?;

    let bucket = df.column("bucket").or_system_err(ADVICE)?.i64().or_system_err(ADVICE)?;
    let pageviews = df.column("pageviews").or_system_err(ADVICE)?.i64().or_system_err(ADVICE)?;
    let visitors = df.column("visitors").or_system_err(ADVICE)?.i64().or_system_err(ADVICE)?;

    Ok((0..df.height())
        .map(|i| TimeSeriesPoint {
            timestamp_ms: bucket.get(i).unwrap_or(0),
            pageviews: pageviews.get(i).unwrap_or(0),
            visitors: visitors.get(i).unwrap_or(0),
        })
        .collect())
}

fn breakdown(pageloads: LazyFrame, column: &str) -> Result<Vec<KeyCount>> {
    let df = pageloads
        .filter(col(column).is_not_null())
        .group_by([col(column)])
        .agg([len().cast(DataType::Int64).alias("count")])
        .sort(["count"], SortMultipleOptions::default().with_order_descending(true))
        .limit(BREAKDOWN_LIMIT)
        .collect()
        .or_system_err(ADVICE)?;

    let keys = df.column(column).or_system_err(ADVICE)?.str().or_system_err(ADVICE)?;
    let counts = df.column("count").or_system_err(ADVICE)?.i64().or_system_err(ADVICE)?;

    Ok((0..df.height())
        .filter_map(|i| {
            keys.get(i).map(|key| KeyCount {
                key: key.to_string(),
                count: counts.get(i).unwrap_or(0),
            })
        })
        .collect())
}

fn per_source_totals(base: LazyFrame) -> Result<Vec<(String, i64, i64)>> {
    let df = base
        .filter(col("kind").eq(lit("page_load")))
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

    let sources = df.column("source").or_system_err(ADVICE)?.str().or_system_err(ADVICE)?;
    let pageviews = df.column("pageviews").or_system_err(ADVICE)?.i64().or_system_err(ADVICE)?;
    let visitors = df.column("visitors").or_system_err(ADVICE)?.i64().or_system_err(ADVICE)?;

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

/// Recursively collect `*.parquet` files under `dir`.
fn parquet_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(parquet_files(&path));
        } else if path.extension().is_some_and(|e| e == "parquet") {
            out.push(path);
        }
    }
    out
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
}
