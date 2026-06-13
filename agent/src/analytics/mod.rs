//! The polars query layer. Statistics are computed over the union of the redb hot
//! store and the cold Parquet partitions, filtered to a project's source URIs and a
//! time range. Queries are CPU-bound and synchronous, so handlers run them via
//! `web::block`.

use std::path::Path;

use analytics_api::{
    KeyCount, MetricSummary, Overview, ProjectSummary, Source, Stats, TimeSeriesPoint,
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

    // Aggregate per-source totals up to projects, and collect unassigned sources.
    let mut project_totals: std::collections::HashMap<String, (i64, i64)> =
        std::collections::HashMap::new();
    let mut unassigned: Vec<KeyCount> = Vec::new();
    let by_uri: std::collections::HashMap<&str, &Source> =
        sources.iter().map(|s| (s.uri.as_str(), s)).collect();

    for (uri, visitors, pageviews) in &per_source {
        match by_uri.get(uri.as_str()).and_then(|s| s.project_id.as_deref()) {
            Some(project_id) => {
                let entry = project_totals.entry(project_id.to_string()).or_insert((0, 0));
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
    project_summaries.sort_by(|a, b| b.pageviews.cmp(&a.pageviews));
    unassigned.sort_by(|a, b| b.count.cmp(&a.count));

    Ok(Overview {
        summary,
        timeseries,
        projects: project_summaries,
        unassigned,
    })
}

/// The source URIs belonging to a project (its sources, and — in Phase 7 — pixels).
pub fn project_source_uris(store: &Store, project_id: &str) -> Result<Vec<String>> {
    Ok(store
        .list_sources()?
        .into_iter()
        .filter(|s| s.project_id.as_deref() == Some(project_id))
        .map(|s| s.uri)
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
