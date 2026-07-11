//! The polars query layer. Statistics are computed over the union of the redb hot
//! store and the cold Parquet partitions, filtered by a compiled [`filter`]
//! expression, and bounded to a half-open `[from, to)` time range. Queries are
//! CPU-bound and synchronous, so handlers run them via `web::block`.

pub mod filter;

use std::collections::HashMap;
use std::path::Path;

use analytics_api::{
    BreakdownRow, Breakdowns, CountRow, Dashboard, EventBreakdowns, EventDetail, EventVariant,
    ExceptionBreakdowns, ExceptionGroup, ExceptionGroupDetail, ExceptionStatus, ExceptionVariant,
    MetricSummary, SessionTrace, TREND_BUCKETS, TimeSeriesPoint, TraceEvent, TraceEventKind,
    TraceSummary, VersionRow, pixel_source, source_label, summary_line,
};
use chrono::{Datelike, TimeZone, Utc};
use polars::prelude::*;
use tracing_batteries::prelude::warn;

use crate::errors::{Result, ResultExt};
use crate::store::Store;

use filter::CompiledFilter;

const ADVICE: &[&str] = &["This is an internal analytics error; please report it with the logs."];

const BREAKDOWN_LIMIT: u32 = 25;
/// How many recent session traces the dashboard payload samples.
const TRACE_SAMPLE: u32 = 10;
/// `[100ms, 5s]` is treated as a bounce (per the medama methodology).
const BOUNCE_MIN_MS: i64 = 100;
const BOUNCE_MAX_MS: i64 = 5_000;
const MIN_BOUNCE_SAMPLES: i64 = 5;

/// The full dashboard payload: headline metrics with a previous-window baseline,
/// the (index-aligned) time series pair, every dimension breakdown, and the
/// project/source rollups — all computed over one filtered scan.
///
/// `filter` is the compiled `q` expression (see [`filter::compile_query`]);
/// `None` means unfiltered.
///
/// The event frame spanning `[from - len, to)` is collected **once** and every
/// aggregation runs against the in-memory frame, so a dashboard request costs a
/// single pass over the Parquet partitions regardless of how many panels it feeds.
pub fn dashboard(
    store: &Store,
    parquet_dir: &str,
    filter: Option<&CompiledFilter>,
    from_ms: i64,
    to_ms: i64,
    bucket_ms: i64,
) -> Result<Dashboard> {
    let len = (to_ms - from_ms).max(1);
    let prev_from = from_ms - len;

    // One scan covers both the current window and the comparison baseline.
    let mut lf = combined(store, parquet_dir, prev_from, to_ms)?;
    if let Some(filter) = filter {
        lf = lf.filter(filter.predicate.clone());
    }
    let df = lf.collect().or_system_err(ADVICE)?;

    let current = df
        .clone()
        .lazy()
        .filter(col("received_ms").gt_eq(lit(from_ms)));
    let previous = df.lazy().filter(col("received_ms").lt(lit(from_ms)));

    // With a path filter active, `is_unique_user` (which rides only on the first
    // page load of a visitor's day) would undercount non-landing pages to ~zero;
    // daily-unique *page* views are the honest visitor count there.
    let unique_flag = if filter.is_some_and(|f| f.references("path")) {
        "is_unique_page"
    } else {
        "is_unique_user"
    };

    // The previous series is computed on the *current* window's bucket grid by
    // shifting events forward one window length, guaranteeing index alignment;
    // timestamps are then shifted back to the previous window's own instants.
    let prev_shifted = previous
        .clone()
        .with_columns([(col("received_ms") + lit(len)).alias("received_ms")]);
    let mut previous_timeseries = timeseries(prev_shifted, from_ms, to_ms, bucket_ms, unique_flag)?;
    for point in &mut previous_timeseries {
        point.timestamp_ms -= len;
    }

    let pageloads = current.clone().filter(col("kind").eq(lit("page_load")));
    let event_names = event_name_breakdown(current.clone().filter(is_event()))?;
    let per_source = source_rollup(current.clone(), unique_flag)?;
    let (projects, sources, unassigned) = project_rollup(store, per_source)?;
    let traces = recent_traces(current.clone(), TRACE_SAMPLE)?;

    Ok(Dashboard {
        summary: summary(current.clone(), unique_flag)?,
        previous_summary: summary(previous, unique_flag)?,
        timeseries: timeseries(current, from_ms, to_ms, bucket_ms, unique_flag)?,
        previous_timeseries,
        breakdowns: Breakdowns {
            pages: breakdown(pageloads.clone(), "pathname", "is_unique_page")?,
            referrers: breakdown(pageloads.clone(), "referrer_host", unique_flag)?,
            countries: breakdown(pageloads.clone(), "country", unique_flag)?,
            languages: breakdown(pageloads.clone(), "language", unique_flag)?,
            browsers: breakdown(pageloads.clone(), "ua_browser", unique_flag)?,
            versions: version_breakdown(pageloads.clone(), unique_flag)?,
            operating_systems: breakdown(pageloads.clone(), "ua_os", unique_flag)?,
            devices: breakdown(pageloads.clone(), "ua_device", unique_flag)?,
            utm_sources: breakdown(pageloads.clone(), "utm_source", unique_flag)?,
            utm_mediums: breakdown(pageloads.clone(), "utm_medium", unique_flag)?,
            utm_campaigns: breakdown(pageloads, "utm_campaign", unique_flag)?,
            event_names,
            projects,
            sources,
        },
        unassigned,
        traces,
    })
}

/// The source URIs belonging to the project whose **name** matches `name`
/// (case-insensitively, matching the filter language's string semantics; names
/// are unique). Values that name no project fall back to an id lookup, so
/// pre-rename links that filtered by project id keep working. An unknown value
/// resolves to no sources, so the filter matches nothing — never everything.
pub fn project_source_uris_by_name(store: &Store, name: &str) -> Result<Vec<String>> {
    let projects = store.list_projects()?;
    let needle = name.to_lowercase();
    let project = projects
        .iter()
        .find(|p| p.name.to_lowercase() == needle)
        .or_else(|| projects.iter().find(|p| p.id == name));
    match project {
        Some(project) => project_source_uris(store, &project.id),
        None => Ok(Vec::new()),
    }
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

/// Exception groups matching the compiled filter, grouped by
/// `(fingerprint, source)` with a [`TREND_BUCKETS`]-bucket occurrence trend
/// each. A fingerprint is computed from the error alone, so the same
/// `exc_group` legitimately occurs on multiple sources/projects; keeping the
/// source in the key keeps those occurrences separate. The caller folds
/// per-source rows up to per-project rows (summing trends element-wise) for
/// the global Exceptions inbox.
pub fn exception_groups_by_source(
    store: &Store,
    parquet_dir: &str,
    from_ms: i64,
    to_ms: i64,
    filter: Option<&CompiledFilter>,
) -> Result<Vec<(ExceptionGroup, String)>> {
    let mut lf = combined(store, parquet_dir, from_ms, to_ms)?
        .filter(col("kind").eq(lit("exception")))
        .filter(col("exc_group").is_not_null());
    if let Some(filter) = filter {
        lf = lf.filter(filter.predicate.clone());
    }
    let df = lf
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
            col("received_ms").alias("times"),
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
    let times = df
        .column("times")
        .or_system_err(ADVICE)?
        .list()
        .or_system_err(ADVICE)?;

    Ok((0..df.height())
        .filter_map(|i| {
            group_id.get(i).map(|gid| {
                (
                    ExceptionGroup {
                        group_id: gid.to_string(),
                        exc_type: exc_type.get(i).unwrap_or("").to_string(),
                        sample_message: summary_line(message.get(i).unwrap_or("")).to_string(),
                        count: count.get(i).unwrap_or(0),
                        first_seen_ms: first.get(i).unwrap_or(0),
                        last_seen_ms: last.get(i).unwrap_or(0),
                        status: ExceptionStatus::Unresolved,
                        resolved: false,
                        muted: false,
                        note: None,
                        trend: trend_of(list_i64(times, i).into_iter(), from_ms, to_ms),
                    },
                    source.get(i).unwrap_or("").to_string(),
                )
            })
        })
        .collect())
}

/// A single exception group in forensic detail: the aggregate (with trend),
/// how its occurrences distribute across key dimensions, and its **distinct
/// variants** — occurrences collapsed by (message, stack, handledness) so an
/// operator scrubs through genuinely different examples rather than paging
/// hundreds of identical ones. Derived from one scan filtered to the group;
/// looked up by id directly (no top-N cap), so a linked or bookmarked group
/// opens regardless of how many fingerprints a project has. Returns `None` if
/// the group has no occurrences in `[from_ms, to_ms)`.
pub fn exception_detail(
    store: &Store,
    parquet_dir: &str,
    sources: &[String],
    group_id: &str,
    from_ms: i64,
    to_ms: i64,
    limit: usize,
) -> Result<Option<ExceptionGroupDetail>> {
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
            col("ua_device"),
            col("app_version"),
            col("source"),
            col("metadata_json"),
            col("sid"),
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
    let received = df
        .column("received_ms")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;

    // Rows are newest-first: index 0 is the most recent occurrence, the last
    // index the oldest. The aggregate spans every row.
    let group = ExceptionGroup {
        group_id: group_id.to_string(),
        exc_type: exc_type.get(0).unwrap_or("").to_string(),
        sample_message: summary_line(message.get(0).unwrap_or("")).to_string(),
        count: height as i64,
        first_seen_ms: received.get(height - 1).unwrap_or(0),
        last_seen_ms: received.get(0).unwrap_or(0),
        status: ExceptionStatus::Unresolved,
        resolved: false,
        muted: false,
        note: None,
        trend: trend_of((0..height).filter_map(|i| received.get(i)), from_ms, to_ms),
    };

    let breakdowns = ExceptionBreakdowns {
        app_versions: app_version_rows(&df)?,
        browsers: count_by(&df, "ua_browser")?,
        operating_systems: count_by(&df, "ua_os")?,
        devices: count_by(&df, "ua_device")?,
    };
    let variants = variants_of(&df, limit)?;

    let traces = traces_of_occurrences(store, parquet_dir, &df, from_ms, to_ms)?;

    Ok(Some(ExceptionGroupDetail {
        group,
        breakdowns,
        variants,
        traces,
    }))
}

/// One named custom/pixel event in forensic detail: the aggregate (with
/// trend), how its occurrences distribute across key dimensions, its
/// **distinct metadata variants** (one representative per unique reporter
/// payload), and the sessions it occurred in. `filter` is the dashboard's
/// compiled `q` expression, so the numbers cover the same slice the operator
/// was looking at. Returns `None` if the event has no occurrences in
/// `[from_ms, to_ms)`.
pub fn event_detail(
    store: &Store,
    parquet_dir: &str,
    name: &str,
    from_ms: i64,
    to_ms: i64,
    filter: Option<&CompiledFilter>,
    limit: usize,
) -> Result<Option<EventDetail>> {
    let mut lf = combined(store, parquet_dir, from_ms, to_ms)?
        .filter(is_event())
        .filter(col("event_name").eq(lit(name.to_string())));
    if let Some(filter) = filter {
        lf = lf.filter(filter.predicate.clone());
    }
    let df = lf
        .select([
            col("received_ms")
                .cast(DataType::Int64)
                .alias("received_ms"),
            col("source"),
            col("pathname"),
            col("country"),
            col("language"),
            col("ua_browser"),
            col("ua_os"),
            col("ua_device"),
            col("metadata_json"),
            col("sid"),
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

    let received = df
        .column("received_ms")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;

    let breakdowns = EventBreakdowns {
        sources: count_by(&df, "source")?,
        pages: count_by(&df, "pathname")?,
        browsers: count_by(&df, "ua_browser")?,
        operating_systems: count_by(&df, "ua_os")?,
        devices: count_by(&df, "ua_device")?,
        countries: count_by(&df, "country")?,
        languages: count_by(&df, "language")?,
    };
    let variants = event_variants(&df, limit)?;
    let traces = traces_of_occurrences(store, parquet_dir, &df, from_ms, to_ms)?;

    Ok(Some(EventDetail {
        name: name.to_string(),
        count: height as i64,
        first_seen_ms: received.get(height - 1).unwrap_or(0),
        last_seen_ms: received.get(0).unwrap_or(0),
        trend: trend_of((0..height).filter_map(|i| received.get(i)), from_ms, to_ms),
        breakdowns,
        variants,
        traces,
    }))
}

/// The sessions an (already newest-first) occurrence frame belonged to, newest
/// first, so the operator can pick which trace to open. A second scan
/// summarizes those sessions in full — their page views and events, not just
/// the occurrences that matched.
fn traces_of_occurrences(
    store: &Store,
    parquet_dir: &str,
    occurrences: &DataFrame,
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<TraceSummary>> {
    let sid = occurrences
        .column("sid")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let mut sids: Vec<String> = Vec::new();
    for i in 0..occurrences.height() {
        if let Some(sid) = sid.get(i)
            && !sid.is_empty()
            && !sids.iter().any(|seen| seen == sid)
        {
            sids.push(sid.to_string());
            if sids.len() >= TRACE_SAMPLE as usize {
                break;
            }
        }
    }
    if sids.is_empty() {
        return Ok(Vec::new());
    }
    let sessions = combined(store, parquet_dir, from_ms, to_ms)?
        .filter(col("sid").is_in(lit(Series::new("sids".into(), sids)).implode(false), false));
    recent_traces(sessions, TRACE_SAMPLE)
}

/// Collapse an event's occurrences (already sorted newest-first) into distinct
/// variants keyed by their reporter metadata: one representative each, counted,
/// most frequent first. The representative context (client, source, page)
/// comes from the variant's latest occurrence.
fn event_variants(occurrences: &DataFrame, limit: usize) -> Result<Vec<EventVariant>> {
    let df = occurrences
        .clone()
        .lazy()
        .group_by([col("metadata_json")])
        .agg([
            len().cast(DataType::Int64).alias("count"),
            col("received_ms").min().alias("first_seen"),
            col("received_ms").max().alias("last_seen"),
            // The frame is newest-first, so `first()` is the latest context.
            col("ua_browser").first().alias("ua_browser"),
            col("ua_os").first().alias("ua_os"),
            col("source").first().alias("source"),
            col("pathname").first().alias("pathname"),
            // The session link: the latest occurrence that has one.
            col("sid").drop_nulls().first().alias("sid"),
        ])
        .sort(
            ["count"],
            SortMultipleOptions::default().with_order_descending(true),
        )
        .limit(limit as u32)
        .collect()
        .or_system_err(ADVICE)?;

    let metadata = df
        .column("metadata_json")
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
    let source = df
        .column("source")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let pathname = df
        .column("pathname")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let sid = df
        .column("sid")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;

    Ok((0..df.height())
        .map(|i| EventVariant {
            metadata: metadata.get(i).map(str::to_string),
            count: count.get(i).unwrap_or(0),
            first_seen_ms: first.get(i).unwrap_or(0),
            last_seen_ms: last.get(i).unwrap_or(0),
            ua_browser: browser.get(i).map(str::to_string),
            ua_os: os.get(i).map(str::to_string),
            source: source.get(i).map(str::to_string),
            pathname: pathname.get(i).map(str::to_string),
            session_id: sid.get(i).map(str::to_string),
        })
        .collect())
}

/// Occurrence counts per value of `column` (nulls under the empty-string
/// sentinel), largest first.
fn count_by(occurrences: &DataFrame, column: &str) -> Result<Vec<CountRow>> {
    let df = occurrences
        .clone()
        .lazy()
        .with_columns([col(column).fill_null(lit("")).alias("key")])
        .group_by([col("key")])
        .agg([len().cast(DataType::Int64).alias("count")])
        .sort(
            ["count"],
            SortMultipleOptions::default().with_order_descending(true),
        )
        .limit(BREAKDOWN_LIMIT)
        .collect()
        .or_system_err(ADVICE)?;

    let keys = df
        .column("key")
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
            keys.get(i).map(|key| CountRow {
                key: key.to_string(),
                count: counts.get(i).unwrap_or(0),
            })
        })
        .collect())
}

/// Occurrence counts per reported release. When the frame spans several
/// sources, rows are keyed as `app @ version` (the app being the source's
/// label) — a release number is only meaningful within its application. A
/// frame scoped to a single source (the per-source detail view) keys rows by
/// the bare version number, since the application is given. Occurrences with
/// no reported version aggregate under the empty sentinel, whichever source
/// they came from.
fn app_version_rows(occurrences: &DataFrame) -> Result<Vec<CountRow>> {
    let df = occurrences
        .clone()
        .lazy()
        .with_columns([
            col("source").fill_null(lit("")).alias("app"),
            col("app_version").fill_null(lit("")).alias("version"),
        ])
        .group_by([col("app"), col("version")])
        .agg([len().cast(DataType::Int64).alias("count")])
        .collect()
        .or_system_err(ADVICE)?;

    let apps = df
        .column("app")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let versions = df
        .column("version")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let counts = df
        .column("count")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;

    // Qualify versions with their application only when the frame genuinely
    // mixes applications; labels are compared (not URIs) since distinct
    // sources can share one (http vs https).
    let mut labels: Vec<&str> = (0..df.height())
        .filter_map(|i| apps.get(i))
        .filter(|app| !app.is_empty())
        .map(source_label)
        .collect();
    labels.sort_unstable();
    labels.dedup();
    let qualify = labels.len() > 1;

    // Fold by the display key rather than trusting the group-by to have
    // finished the job (see the label-sharing note above).
    let mut totals: HashMap<String, i64> = HashMap::new();
    for i in 0..df.height() {
        let (Some(app), Some(version)) = (apps.get(i), versions.get(i)) else {
            continue;
        };
        let key = match (app.is_empty(), version.is_empty()) {
            (_, true) => String::new(),
            (true, false) => version.to_string(),
            (false, false) if qualify => format!("{} @ {version}", source_label(app)),
            (false, false) => version.to_string(),
        };
        *totals.entry(key).or_insert(0) += counts.get(i).unwrap_or(0);
    }
    let mut rows: Vec<CountRow> = totals
        .into_iter()
        .map(|(key, count)| CountRow { key, count })
        .collect();
    rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
    rows.truncate(BREAKDOWN_LIMIT as usize);
    Ok(rows)
}

/// Collapse a group's occurrences (already sorted newest-first) into distinct
/// variants keyed by (message, stack, handledness): one representative each,
/// counted, most frequent first. The representative context (client, source,
/// version, reporter metadata) comes from the variant's latest occurrence.
fn variants_of(occurrences: &DataFrame, limit: usize) -> Result<Vec<ExceptionVariant>> {
    let df = occurrences
        .clone()
        .lazy()
        .group_by([col("exc_message"), col("exc_stack"), col("exc_handled")])
        .agg([
            len().cast(DataType::Int64).alias("count"),
            col("received_ms").min().alias("first_seen"),
            col("received_ms").max().alias("last_seen"),
            // The frame is newest-first, so `first()` is the latest context.
            col("ua_browser").first().alias("ua_browser"),
            col("ua_os").first().alias("ua_os"),
            col("source").first().alias("source"),
            col("app_version").first().alias("app_version"),
            // Metadata is optional per report; surface the latest occurrence
            // that actually carried some.
            col("metadata_json")
                .drop_nulls()
                .first()
                .alias("metadata_json"),
            // Likewise the session link: the latest occurrence that has one.
            col("sid").drop_nulls().first().alias("sid"),
        ])
        .sort(
            ["count"],
            SortMultipleOptions::default().with_order_descending(true),
        )
        .limit(limit as u32)
        .collect()
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
    let source = df
        .column("source")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let version = df
        .column("app_version")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let metadata = df
        .column("metadata_json")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let sid = df
        .column("sid")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;

    Ok((0..df.height())
        .map(|i| ExceptionVariant {
            message: message.get(i).unwrap_or("").to_string(),
            stack: stack.get(i).map(str::to_string),
            handled: handled.get(i).unwrap_or(false),
            count: count.get(i).unwrap_or(0),
            first_seen_ms: first.get(i).unwrap_or(0),
            last_seen_ms: last.get(i).unwrap_or(0),
            ua_browser: browser.get(i).map(str::to_string),
            ua_os: os.get(i).map(str::to_string),
            source: source.get(i).map(str::to_string),
            app_version: version.get(i).map(str::to_string),
            metadata: metadata.get(i).map(str::to_string),
            session_id: sid.get(i).map(str::to_string),
        })
        .collect())
}

/// The most recent sessions in the (already filtered) event frame, one summary
/// row each, newest first. Events are grouped by the tracker's per-visit
/// session id; events without one (pixel hits, pre-session trackers) never
/// form a trace. The summary spans the *matching* events only, so a dimension
/// filter scopes this list exactly the way it scopes every other panel.
fn recent_traces(base: LazyFrame, limit: u32) -> Result<Vec<TraceSummary>> {
    let df = base
        .filter(col("sid").is_not_null().and(col("sid").neq(lit(""))))
        // Frame order feeds the `first()` aggregations below: oldest first
        // makes them "the session's earliest value".
        .sort(["received_ms"], SortMultipleOptions::default())
        .group_by([col("sid")])
        .agg([
            col("received_ms")
                .min()
                .cast(DataType::Int64)
                .alias("started"),
            col("received_ms").max().cast(DataType::Int64).alias("last"),
            col("source").first().alias("source"),
            // The first page viewed in the session.
            col("pathname")
                .filter(col("kind").eq(lit("page_load")))
                .drop_nulls()
                .first()
                .alias("entry_path"),
            col("country").drop_nulls().first().alias("country"),
            col("ua_browser").drop_nulls().first().alias("ua_browser"),
            col("ua_version").drop_nulls().first().alias("ua_version"),
            col("ua_device").drop_nulls().first().alias("ua_device"),
            // Exception reports carry the app's claimed release.
            col("app_version").drop_nulls().first().alias("app_version"),
            col("kind")
                .eq(lit("page_load"))
                .sum()
                .cast(DataType::Int64)
                .alias("pageviews"),
            is_event().sum().cast(DataType::Int64).alias("events"),
            col("kind")
                .eq(lit("exception"))
                .sum()
                .cast(DataType::Int64)
                .alias("exceptions"),
        ])
        .sort(
            ["started"],
            SortMultipleOptions::default().with_order_descending(true),
        )
        .limit(limit)
        .collect()
        .or_system_err(ADVICE)?;

    let sid = df
        .column("sid")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let started = df
        .column("started")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let last = df
        .column("last")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let source = df
        .column("source")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let entry_path = df
        .column("entry_path")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let country = df
        .column("country")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let ua_browser = df
        .column("ua_browser")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let ua_version = df
        .column("ua_version")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let ua_device = df
        .column("ua_device")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let app_version = df
        .column("app_version")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let pageviews = df
        .column("pageviews")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let events = df
        .column("events")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let exceptions = df
        .column("exceptions")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;

    Ok((0..df.height())
        .filter_map(|i| {
            sid.get(i).map(|session_id| TraceSummary {
                session_id: session_id.to_string(),
                started_ms: started.get(i).unwrap_or(0),
                last_ms: last.get(i).unwrap_or(0),
                source: source.get(i).unwrap_or("").to_string(),
                entry_path: entry_path.get(i).map(str::to_string),
                country: country.get(i).map(str::to_string),
                ua_browser: ua_browser.get(i).map(str::to_string),
                ua_version: ua_version.get(i).map(str::to_string),
                ua_device: ua_device.get(i).map(str::to_string),
                app_version: app_version.get(i).map(str::to_string),
                pageviews: pageviews.get(i).unwrap_or(0),
                events: events.get(i).unwrap_or(0),
                exceptions: exceptions.get(i).unwrap_or(0),
            })
        })
        .collect())
}

/// One session's full timeline: every event carrying the session id, oldest
/// first, plus the visit's context (source, locale, client, claimed release)
/// drawn from the earliest event that reports each. Looked up by id directly —
/// no recency cap — so a trace linked from an exception exemplar or a bookmark
/// always opens; `limit` bounds the returned timeline. Returns `None` when the
/// session has no events in `[from_ms, to_ms)`.
pub fn session_trace(
    store: &Store,
    parquet_dir: &str,
    session_id: &str,
    from_ms: i64,
    to_ms: i64,
    limit: usize,
) -> Result<Option<SessionTrace>> {
    let df = combined(store, parquet_dir, from_ms, to_ms)?
        .filter(col("sid").eq(lit(session_id.to_string())))
        .select([
            col("received_ms")
                .cast(DataType::Int64)
                .alias("received_ms"),
            col("seq"),
            col("kind"),
            col("bid"),
            col("source"),
            col("pathname"),
            col("country"),
            col("language"),
            col("ua_browser"),
            col("ua_version"),
            col("ua_os"),
            col("app_version"),
            col("duration_ms"),
            col("event_name"),
            col("metadata_json"),
            col("exc_type"),
            col("exc_message"),
            col("exc_stack"),
            col("exc_group"),
            col("exc_handled"),
        ])
        // `seq` breaks same-millisecond ties in arrival order.
        .sort(["received_ms", "seq"], SortMultipleOptions::default())
        .limit(limit as u32)
        .collect()
        .or_system_err(ADVICE)?;

    let height = df.height();
    if height == 0 {
        return Ok(None);
    }

    let received = df
        .column("received_ms")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let kind = df
        .column("kind")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let bid = df
        .column("bid")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let pathname = df
        .column("pathname")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let duration = df
        .column("duration_ms")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let event_name = df
        .column("event_name")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let metadata = df
        .column("metadata_json")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let exc_type = df
        .column("exc_type")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let exc_message = df
        .column("exc_message")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let exc_stack = df
        .column("exc_stack")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let exc_group = df
        .column("exc_group")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let exc_handled = df
        .column("exc_handled")
        .or_system_err(ADVICE)?
        .bool()
        .or_system_err(ADVICE)?;

    let events: Vec<TraceEvent> = (0..height)
        .filter_map(|i| {
            let kind = match kind.get(i) {
                Some("page_load") => TraceEventKind::PageLoad,
                Some("page_unload") => TraceEventKind::PageUnload,
                Some("custom") => TraceEventKind::Custom,
                Some("exception") => TraceEventKind::Exception,
                // Pixels carry no session; anything else has no place on a trace.
                _ => return None,
            };
            Some(TraceEvent {
                received_ms: received.get(i).unwrap_or(0),
                kind,
                bid: bid.get(i).unwrap_or("").to_string(),
                pathname: pathname.get(i).map(str::to_string),
                duration_ms: duration.get(i),
                event_name: event_name.get(i).map(str::to_string),
                metadata: metadata.get(i).map(str::to_string),
                exc_type: exc_type.get(i).map(str::to_string),
                exc_message: exc_message.get(i).map(str::to_string),
                exc_stack: exc_stack.get(i).map(str::to_string),
                exc_group: exc_group.get(i).map(str::to_string),
                exc_handled: exc_handled.get(i),
            })
        })
        .collect();

    // The visit's context: the earliest non-null value of each dimension (a
    // session is one client on one source, so any row would do — but events
    // differ in which columns they carry).
    let first_str = |name: &str| -> Result<Option<String>> {
        let column = df
            .column(name)
            .or_system_err(ADVICE)?
            .str()
            .or_system_err(ADVICE)?
            .clone();
        Ok((0..height).find_map(|i| column.get(i).map(str::to_string)))
    };

    Ok(Some(SessionTrace {
        session_id: session_id.to_string(),
        started_ms: received.get(0).unwrap_or(0),
        ended_ms: received.get(height - 1).unwrap_or(0),
        source: first_str("source")?.unwrap_or_default(),
        country: first_str("country")?,
        language: first_str("language")?,
        ua_browser: first_str("ua_browser")?,
        ua_version: first_str("ua_version")?,
        ua_os: first_str("ua_os")?,
        app_version: first_str("app_version")?,
        events,
    }))
}

// ----------------------------------------------------------------- internals

/// Occurrence timestamps bucketed into [`TREND_BUCKETS`] equal slices of
/// `[from, to)`, oldest first.
fn trend_of(times: impl Iterator<Item = i64>, from_ms: i64, to_ms: i64) -> Vec<i64> {
    let span = (to_ms - from_ms).max(1) as i128;
    let mut buckets = vec![0i64; TREND_BUCKETS];
    for t in times {
        let idx = ((t - from_ms) as i128 * TREND_BUCKETS as i128 / span)
            .clamp(0, TREND_BUCKETS as i128 - 1) as usize;
        buckets[idx] += 1;
    }
    buckets
}

/// The `i64` values of row `i` of a list column.
fn list_i64(column: &ListChunked, i: usize) -> Vec<i64> {
    let Some(series) = column.get_as_series(i) else {
        return Vec::new();
    };
    let Ok(values) = series.i64() else {
        return Vec::new();
    };
    (0..values.len()).filter_map(|j| values.get(j)).collect()
}

/// The time-filtered (half-open `[from, to)`) union of the cold Parquet
/// partitions and the redb hot store.
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
        // Diagonal: partitions written before a column existed (e.g. the app
        // attribution columns) read back with that column as nulls instead of
        // failing the union.
        concat(
            frames,
            UnionArgs {
                to_supertypes: true,
                diagonal: true,
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
                .and(col("received_ms").lt(lit(to_ms))),
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

/// Pixel hits and custom application events (counted as `events`, not pageviews).
fn is_event() -> Expr {
    col("kind")
        .eq(lit("pixel"))
        .or(col("kind").eq(lit("custom")))
}

fn summary(base: LazyFrame, unique_flag: &str) -> Result<MetricSummary> {
    let df = base
        .select([
            col("kind")
                .eq(lit("page_load"))
                .sum()
                .cast(DataType::Int64)
                .alias("pageviews"),
            col("kind")
                .eq(lit("page_load"))
                .and(col(unique_flag))
                .sum()
                .cast(DataType::Int64)
                .alias("visitors"),
            is_event().sum().cast(DataType::Int64).alias("events"),
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
        events: scalar_i64(&df, "events"),
        bounce_rate: (samples >= MIN_BOUNCE_SAMPLES).then(|| bounces as f64 / samples as f64),
        median_duration_ms: median.map(|m| m.round() as i64),
    })
}

/// A continuous time series over `[from_ms, to_ms)` at `bucket_ms` resolution.
/// Buckets with no events are emitted as zeros so the chart shows a gap-free line
/// across the whole window instead of collapsing absent periods.
fn timeseries(
    base: LazyFrame,
    from_ms: i64,
    to_ms: i64,
    bucket_ms: i64,
    unique_flag: &str,
) -> Result<Vec<TimeSeriesPoint>> {
    let bucket_ms = bucket_ms.max(1);
    let is_exception = col("kind").eq(lit("exception"));
    let df = base
        .filter(
            col("kind")
                .eq(lit("page_load"))
                .or(is_event())
                .or(is_exception.clone()),
        )
        .with_columns([(col("received_ms") - col("received_ms") % lit(bucket_ms)).alias("bucket")])
        .group_by([col("bucket")])
        .agg([
            col("kind")
                .eq(lit("page_load"))
                .sum()
                .cast(DataType::Int64)
                .alias("pageviews"),
            col("kind")
                .eq(lit("page_load"))
                .and(col(unique_flag))
                .sum()
                .cast(DataType::Int64)
                .alias("visitors"),
            is_event().sum().cast(DataType::Int64).alias("events"),
            is_exception.sum().cast(DataType::Int64).alias("exceptions"),
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
    let events = df
        .column("events")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;
    let exceptions = df
        .column("exceptions")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;

    // Index the populated buckets, then walk every bucket in the window.
    let mut counts: HashMap<i64, (i64, i64, i64, i64)> = HashMap::new();
    for i in 0..df.height() {
        if let Some(b) = bucket.get(i) {
            counts.insert(
                b,
                (
                    pageviews.get(i).unwrap_or(0),
                    visitors.get(i).unwrap_or(0),
                    events.get(i).unwrap_or(0),
                    exceptions.get(i).unwrap_or(0),
                ),
            );
        }
    }

    let point = |timestamp_ms: i64,
                 (pageviews, visitors, events, exceptions): (i64, i64, i64, i64)| {
        TimeSeriesPoint {
            timestamp_ms,
            pageviews,
            visitors,
            events,
            exceptions,
        }
    };

    let first = from_ms - from_ms.rem_euclid(bucket_ms);
    let last = (to_ms - 1) - (to_ms - 1).rem_euclid(bucket_ms);
    // Guard against a pathological window/bucket combination producing a huge vec.
    let estimated = ((last - first) / bucket_ms).unsigned_abs() as usize + 1;
    if first > last || estimated > 5_000 {
        // Fall back to the populated buckets only (sorted).
        let mut points: Vec<TimeSeriesPoint> = counts
            .into_iter()
            .map(|(b, tuple)| point(b, tuple))
            .collect();
        points.sort_by_key(|p| p.timestamp_ms);
        return Ok(points);
    }

    let mut points = Vec::with_capacity(estimated);
    let mut b = first;
    while b <= last {
        points.push(point(b, counts.get(&b).copied().unwrap_or((0, 0, 0, 0))));
        b += bucket_ms;
    }
    Ok(points)
}

/// A dimension breakdown over the page-load frame. Null (and empty) dimension
/// values aggregate under the sentinel empty-string key rather than being
/// dropped, so direct traffic and unknown values stay visible and filterable and
/// share percentages stay honest.
fn breakdown(pageloads: LazyFrame, column: &str, unique_flag: &str) -> Result<Vec<BreakdownRow>> {
    let df = pageloads
        .with_columns([col(column).fill_null(lit("")).alias("key")])
        .group_by([col("key")])
        .agg([
            len().cast(DataType::Int64).alias("pageviews"),
            col(unique_flag)
                .sum()
                .cast(DataType::Int64)
                .alias("visitors"),
        ])
        .sort(
            ["pageviews"],
            SortMultipleOptions::default().with_order_descending(true),
        )
        .limit(BREAKDOWN_LIMIT)
        .collect()
        .or_system_err(ADVICE)?;

    let keys = df
        .column("key")
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
            keys.get(i).map(|key| BreakdownRow {
                key: key.to_string(),
                pageviews: pageviews.get(i).unwrap_or(0),
                visitors: visitors.get(i).unwrap_or(0),
                events: 0,
            })
        })
        .collect())
}

/// The client-versions breakdown over the page-load frame, keyed by the
/// (application, version) pair — a version number is only meaningful within
/// its application, so "120.0" from Chrome and "120.0" from Edge stay separate
/// rows. Nulls aggregate under the empty-string sentinel like every other
/// breakdown.
fn version_breakdown(pageloads: LazyFrame, unique_flag: &str) -> Result<Vec<VersionRow>> {
    let df = pageloads
        .with_columns([
            col("ua_browser").fill_null(lit("")).alias("app"),
            col("ua_version").fill_null(lit("")).alias("version"),
        ])
        .group_by([col("app"), col("version")])
        .agg([
            len().cast(DataType::Int64).alias("pageviews"),
            col(unique_flag)
                .sum()
                .cast(DataType::Int64)
                .alias("visitors"),
        ])
        .sort(
            ["pageviews"],
            SortMultipleOptions::default().with_order_descending(true),
        )
        .limit(BREAKDOWN_LIMIT)
        .collect()
        .or_system_err(ADVICE)?;

    let apps = df
        .column("app")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let versions = df
        .column("version")
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
            Some(VersionRow {
                app: apps.get(i)?.to_string(),
                version: versions.get(i)?.to_string(),
                pageviews: pageviews.get(i).unwrap_or(0),
                visitors: visitors.get(i).unwrap_or(0),
                events: 0,
            })
        })
        .collect())
}

/// The custom/pixel events breakdown, keyed by event name (unnamed events
/// aggregate under the empty sentinel). Only the `events` count is meaningful —
/// these rows have no page views, and visitor uniqueness rides on page loads —
/// so the panel displays them under the Events metric.
fn event_name_breakdown(events: LazyFrame) -> Result<Vec<BreakdownRow>> {
    let df = events
        .with_columns([col("event_name").fill_null(lit("")).alias("key")])
        .group_by([col("key")])
        .agg([len().cast(DataType::Int64).alias("events")])
        .sort(
            ["events"],
            SortMultipleOptions::default().with_order_descending(true),
        )
        .limit(BREAKDOWN_LIMIT)
        .collect()
        .or_system_err(ADVICE)?;

    let keys = df
        .column("key")
        .or_system_err(ADVICE)?
        .str()
        .or_system_err(ADVICE)?;
    let events = df
        .column("events")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;

    Ok((0..df.height())
        .filter_map(|i| {
            keys.get(i).map(|key| BreakdownRow {
                key: key.to_string(),
                visitors: 0,
                pageviews: 0,
                events: events.get(i).unwrap_or(0),
            })
        })
        .collect())
}

/// Per-source totals. Page loads count as `pageviews`; pixel hits and custom
/// events count as `events` so pixel-only and application sources still surface;
/// `visitors` uses the same daily-unique flag as every other aggregation in the
/// response (only page loads carry it), so the panels agree with the headline.
fn source_rollup(base: LazyFrame, unique_flag: &str) -> Result<Vec<BreakdownRow>> {
    let df = base
        .filter(col("kind").eq(lit("page_load")).or(is_event()))
        .group_by([col("source")])
        .agg([
            col("kind")
                .eq(lit("page_load"))
                .sum()
                .cast(DataType::Int64)
                .alias("pageviews"),
            col(unique_flag)
                .sum()
                .cast(DataType::Int64)
                .alias("visitors"),
            is_event().sum().cast(DataType::Int64).alias("events"),
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
    let events = df
        .column("events")
        .or_system_err(ADVICE)?
        .i64()
        .or_system_err(ADVICE)?;

    let mut rows: Vec<BreakdownRow> = (0..df.height())
        .filter_map(|i| {
            sources.get(i).map(|s| BreakdownRow {
                key: s.to_string(),
                pageviews: pageviews.get(i).unwrap_or(0),
                visitors: visitors.get(i).unwrap_or(0),
                events: events.get(i).unwrap_or(0),
            })
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.pageviews + r.events));
    Ok(rows)
}

/// Fold per-source totals up to per-project rows, and split off the sources that
/// belong to no project (the operator's "assign these" inbox). Also returns the
/// per-source rows themselves, capped like every other breakdown.
fn project_rollup(
    store: &Store,
    per_source: Vec<BreakdownRow>,
) -> Result<(Vec<BreakdownRow>, Vec<BreakdownRow>, Vec<BreakdownRow>)> {
    // Build a source-URI -> project map from assigned sources and pixels.
    let mut uri_project: HashMap<String, String> = HashMap::new();
    for source in store.list_sources()? {
        if let Some(project_id) = source.project_id {
            uri_project.insert(source.uri, project_id);
        }
    }
    for pixel in store.list_pixels()? {
        uri_project.insert(pixel_source(&pixel.id), pixel.project_id);
    }

    let mut totals: HashMap<String, BreakdownRow> = HashMap::new();
    let mut unassigned: Vec<BreakdownRow> = Vec::new();
    for row in &per_source {
        match uri_project.get(&row.key) {
            Some(project_id) => {
                let entry = totals
                    .entry(project_id.clone())
                    .or_insert_with(|| BreakdownRow {
                        key: project_id.clone(),
                        visitors: 0,
                        pageviews: 0,
                        events: 0,
                    });
                entry.visitors += row.visitors;
                entry.pageviews += row.pageviews;
                entry.events += row.events;
            }
            None => unassigned.push(row.clone()),
        }
    }

    // Every project appears, even with zero traffic in the window, so the panel
    // doubles as the project directory.
    let mut projects: Vec<BreakdownRow> = store
        .list_projects()?
        .into_iter()
        .map(|project| {
            totals.remove(&project.id).unwrap_or(BreakdownRow {
                key: project.id,
                visitors: 0,
                pageviews: 0,
                events: 0,
            })
        })
        .collect();
    projects.sort_by_key(|r| std::cmp::Reverse(r.pageviews + r.events));
    unassigned.sort_by_key(|r| std::cmp::Reverse(r.pageviews + r.events));

    let mut sources = per_source;
    sources.truncate(BREAKDOWN_LIMIT as usize);

    Ok((projects, sources, unassigned))
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

/// The timestamp of the earliest stored event, or `None` when nothing has been
/// recorded yet. Used to resolve "all time" queries (`from=0`) to the real
/// start of the data, so the time series isn't padded with decades of empty
/// buckets. The cold archive is date-partitioned, so its earliest partition
/// directory answers without scanning any data; only a store with no cold
/// partitions yet (first hours of a deployment) reads the hot store.
pub fn earliest_event_ms(store: &Store, parquet_dir: &str) -> Result<Option<i64>> {
    if let Some(ms) = earliest_partition_ms(Path::new(parquet_dir)) {
        return Ok(Some(ms));
    }
    let df = store.hot_dataframe()?;
    Ok(df
        .column("received_ms")
        .ok()
        .and_then(|c| c.i64().ok())
        .and_then(|a| a.min()))
}

/// The UTC-midnight instant of the earliest `YYYY/MM/DD` partition directory,
/// if any exist.
fn earliest_partition_ms(dir: &Path) -> Option<i64> {
    let year = numeric_subdirs::<i32>(dir).into_iter().min()?;
    let year_dir = dir.join(format!("{year:04}"));
    let month = numeric_subdirs::<u32>(&year_dir).into_iter().min()?;
    let month_dir = year_dir.join(format!("{month:02}"));
    let day = numeric_subdirs::<u32>(&month_dir).into_iter().min()?;
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
        .single()
        .map(|dt| dt.timestamp_millis())
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
            is_unique_page: unique,
            ua_browser: Some("Chrome".into()),
            ua_version: Some("120.0".into()),
            duration_ms: duration,
            ..Default::default()
        }
    }

    /// Compile a dashboard `q` expression (panics on error — tests only).
    fn dash_filter(store: &Store, q: &str) -> CompiledFilter {
        filter::compile_query(q, filter::FieldSet::Dashboard, store)
            .unwrap()
            .unwrap()
    }

    fn source_q(source: &str) -> String {
        format!(r#"source == "{source}""#)
    }

    #[test]
    fn earliest_event_ms_prefers_the_cold_archive_and_falls_back_to_hot() {
        let parquet_dir = std::env::temp_dir().join(format!(
            "analytics-earliest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        // With no partitions and an empty hot store there is no earliest event.
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        assert_eq!(
            earliest_event_ms(&store, parquet_dir.to_str().unwrap()).unwrap(),
            None
        );

        // Hot-only: the earliest hot event answers.
        store
            .append_events(&[load("https://a.com", 5_000, true, None)])
            .unwrap();
        assert_eq!(
            earliest_event_ms(&store, parquet_dir.to_str().unwrap()).unwrap(),
            Some(5_000)
        );

        // A date partition (even an empty directory tree counts — partitions
        // only exist once written) beats the hot store.
        std::fs::create_dir_all(parquet_dir.join("2024/03/07")).unwrap();
        std::fs::create_dir_all(parquet_dir.join("2025/01/01")).unwrap();
        let expected = Utc
            .with_ymd_and_hms(2024, 3, 7, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            earliest_event_ms(&store, parquet_dir.to_str().unwrap()).unwrap(),
            Some(expected)
        );
        std::fs::remove_dir_all(&parquet_dir).ok();
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
        let filter = dash_filter(&store, &source_q("https://a.com"));
        let dash = dashboard(
            &store,
            "/nonexistent-parquet",
            Some(&filter),
            0,
            10_000,
            86_400_000,
        )
        .unwrap();

        assert_eq!(dash.summary.pageviews, 3);
        assert_eq!(dash.summary.visitors, 2); // two unique loads for a.com
        assert_eq!(
            dash.breakdowns.pages.first().map(|p| p.key.as_str()),
            Some("/home")
        );
        assert_eq!(dash.breakdowns.pages.first().map(|p| p.pageviews), Some(3));
        assert_eq!(
            dash.breakdowns
                .versions
                .first()
                .map(|v| (v.app.as_str(), v.version.as_str())),
            Some(("Chrome", "120.0"))
        );

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

        let filter = dash_filter(&store, &source_q("https://a.com"));
        let dash = dashboard(&store, "/none", Some(&filter), 0, 3 * day, day).unwrap();

        // Buckets at 0, 1d, 2d — empty days filled with zeros, not dropped
        // (the range is half-open, so the bucket at 3d is not included).
        assert_eq!(dash.timeseries.len(), 3);
        assert_eq!(dash.timeseries[0].pageviews, 2);
        assert!(
            dash.timeseries[1..]
                .iter()
                .all(|p| p.pageviews == 0 && p.visitors == 0)
        );

        // The comparison series is index-aligned: identical length, shifted stamps.
        assert_eq!(dash.previous_timeseries.len(), dash.timeseries.len());
        for (prev, cur) in dash.previous_timeseries.iter().zip(&dash.timeseries) {
            assert_eq!(prev.timestamp_ms, cur.timestamp_ms - 3 * day);
        }

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn previous_window_feeds_summary_not_current() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        // One view in the previous window, two in the current one. The event at
        // exactly `from` belongs to the current window only (half-open ranges).
        store
            .append_events(&[
                load("https://a.com", 4_000, true, None),
                load("https://a.com", 10_000, true, None),
                load("https://a.com", 12_000, false, None),
            ])
            .unwrap();

        let filter = dash_filter(&store, &source_q("https://a.com"));
        let dash = dashboard(&store, "/none", Some(&filter), 10_000, 20_000, 86_400_000).unwrap();

        assert_eq!(dash.summary.pageviews, 2);
        assert_eq!(dash.previous_summary.pageviews, 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn query_expressions_scope_everything() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let mut firefox = load("https://a.com", 2_000, false, None);
        firefox.ua_browser = Some("Firefox".into());
        let mut direct = load("https://a.com", 3_000, false, None);
        direct.ua_browser = None;
        store
            .append_events(&[load("https://a.com", 1_000, true, None), firefox, direct])
            .unwrap();

        // Equality is case-insensitive, mirroring the filter language.
        let filter = dash_filter(&store, r#"browser == "chrome""#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);
        assert_eq!(dash.summary.visitors, 1);

        // Disjunction spans values.
        let filter = dash_filter(&store, r#"browser == "Chrome" || browser == "Firefox""#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 2);

        // Membership lists work too.
        let filter = dash_filter(&store, r#"browser in ["chrome", "firefox"]"#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 2);

        // An empty value matches events where the dimension is absent.
        let filter = dash_filter(&store, r#"browser == """#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);
        assert_eq!(dash.summary.visitors, 0);

        // The absent value surfaces as a sentinel row rather than being dropped.
        let dash = dashboard(&store, "/none", None, 0, 10_000, 86_400_000).unwrap();
        let sentinel = dash.breakdowns.browsers.iter().find(|r| r.key.is_empty());
        assert_eq!(sentinel.map(|r| r.pageviews), Some(1));

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn source_filter_matches_bare_hostnames() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[load("https://a.com", 1_000, true, None)])
            .unwrap();

        let filter = dash_filter(&store, r#"source == "a.com""#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn source_membership_selects_multiple_sources() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                load("https://b.com", 2_000, true, None),
                load("https://c.com", 3_000, true, None), // excluded
                typed("pixel://p1", 4_000, EventKind::Pixel),
            ])
            .unwrap();

        // Bare hostnames expand to every canonical URI form.
        let filter = dash_filter(&store, r#"source in ["a.com", "b.com", "p1"]"#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 2);
        assert_eq!(dash.summary.events, 1); // the pixel matched via pixel://p1
        assert!(
            dash.breakdowns
                .sources
                .iter()
                .all(|r| r.key != "https://c.com")
        );

        // Mixed bare and fully-qualified names work too.
        let filter = dash_filter(&store, r#"source in ["https://a.com", "b.com"]"#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 2);
        assert_eq!(dash.summary.events, 0);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn sentinel_filter_excludes_pixel_and_custom_events() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let mut no_browser = load("https://a.com", 1_000, false, None);
        no_browser.ua_browser = None;
        store
            .append_events(&[
                load("https://a.com", 2_000, true, None), // Chrome
                no_browser,
                typed("pixel://p1", 3_000, EventKind::Pixel),
            ])
            .unwrap();

        // browser == "" (absent) must match the browserless page view but NOT
        // the pixel hit, whose dimensions are null for a different reason.
        let filter = dash_filter(&store, r#"browser == """#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);
        assert_eq!(dash.summary.events, 0);
        assert!(
            dash.breakdowns
                .sources
                .iter()
                .all(|r| r.key != "pixel://p1")
        );

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn path_filter_switches_rollup_visitors_too() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        // A non-landing page view: not the visitor's first load of the day
        // (is_unique_user=false) but the first view of that page.
        let mut blog = load("https://a.com", 1_000, false, None);
        blog.pathname = Some("/blog".into());
        blog.is_unique_page = true;
        store.append_events(&[blog]).unwrap();

        let filter = dash_filter(&store, r#"path == "/blog""#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        // The sources rollup must agree with the headline visitor count.
        assert_eq!(dash.summary.visitors, 1);
        let source = dash.breakdowns.sources.first().expect("source row");
        assert_eq!(source.visitors, 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn path_filter_counts_unique_page_views_as_visitors() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        // A visitor lands on / (daily-unique) then reads /blog: the /blog view is
        // not their first of the day (is_unique_user=false) but *is* the first
        // view of that page (is_unique_page=true).
        let mut landing = load("https://a.com", 1_000, true, None);
        landing.pathname = Some("/".into());
        let mut blog = load("https://a.com", 2_000, false, None);
        blog.pathname = Some("/blog".into());
        blog.is_unique_page = true;
        store.append_events(&[landing, blog]).unwrap();

        let filter = dash_filter(&store, r#"path == "/blog""#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);
        // is_unique_user would report 0 here; is_unique_page reports the truth.
        assert_eq!(dash.summary.visitors, 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn project_with_no_sources_sees_no_traffic() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[load("https://a.com", 1_000, true, None)])
            .unwrap();

        let filter = dash_filter(&store, r#"project == "empty-project""#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 0);
        assert_eq!(dash.summary.visitors, 0);
        assert!(dash.breakdowns.sources.is_empty());

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn project_filter_resolves_names_case_insensitively_with_id_fallback() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .put_project(&analytics_api::Project {
                id: "01ARZAPPS".into(),
                name: "Apps".into(),
                slug: "apps".into(),
                created_at: Utc::now(),
            })
            .unwrap();
        store
            .put_source(&analytics_api::Source {
                uri: "https://a.com".into(),
                project_id: Some("01ARZAPPS".into()),
                kind: analytics_api::default_kind("https://a.com"),
                display_name: None,
                created_at: Utc::now(),
                first_seen: None,
                last_seen: None,
            })
            .unwrap();
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                load("https://b.com", 2_000, true, None), // unassigned, excluded
            ])
            .unwrap();

        // The (unique) name selects the project in any case; the id still
        // resolves so pre-rename links keep working.
        for q in [
            r#"project == "Apps""#,
            r#"project == "APPS""#,
            r#"project in ["Apps"]"#,
            r#"project == "01ARZAPPS""#,
        ] {
            let filter = dash_filter(&store, q);
            let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
            assert_eq!(dash.summary.pageviews, 1, "query `{q}`");
        }

        // Negation excludes the project's traffic but keeps everything else.
        let filter = dash_filter(&store, r#"project != "Apps""#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.summary.pageviews, 1);
        assert!(
            dash.breakdowns
                .sources
                .iter()
                .all(|r| r.key != "https://a.com")
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
    fn exception_groups_keep_sources_separate_and_carry_trends() {
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

        let rows = exception_groups_by_source(&store, "/none", 0, 10_000, None).unwrap();
        // One row per (fingerprint, source) — not collapsed across sources.
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(g, _)| g.group_id == "g1"));
        let a = rows.iter().find(|(_, s)| s == "https://a.com").unwrap();
        assert_eq!(a.0.count, 2);
        assert_eq!(a.0.trend.len(), TREND_BUCKETS);
        assert_eq!(a.0.trend.iter().sum::<i64>(), 2);
        let b = rows.iter().find(|(_, s)| s == "https://b.com").unwrap();
        assert_eq!(b.0.count, 1);
        assert_eq!(b.0.trend.iter().sum::<i64>(), 1);

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

        let filter = dash_filter(&store, &source_q("https://a.com"));
        let dash = dashboard(
            &store,
            parquet_dir.to_str().unwrap(),
            Some(&filter),
            0,
            10_000,
            86_400_000,
        )
        .unwrap();

        // Without dedup this would double to 4 pageviews / 2 visitors.
        assert_eq!(dash.summary.pageviews, 2);
        assert_eq!(dash.summary.visitors, 1);

        drop(store);
        let _ = std::fs::remove_file(&redb);
        let _ = std::fs::remove_dir_all(&parquet_dir);
    }

    #[test]
    fn exception_group_lookup_ignores_the_recency_cap() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let events: Vec<_> = (1..=505)
            .map(|i| exc(&format!("g{i}"), i * 1_000))
            .collect();
        store.append_events(&events).unwrap();
        let sources = ["https://a.com".to_string()];

        // g1 is the oldest, so it falls outside the top-500-by-recency listing...
        let listing_filter = filter::compile_query(
            &source_q("https://a.com"),
            filter::FieldSet::Exceptions,
            &store,
        )
        .unwrap()
        .unwrap();
        let listed =
            exception_groups_by_source(&store, "/none", 0, 10_000_000, Some(&listing_filter))
                .unwrap();
        assert_eq!(listed.len(), 500);
        assert!(!listed.iter().any(|(g, _)| g.group_id == "g1"));

        // ...but a direct lookup still resolves it (group + variants in one scan).
        let g1 = exception_detail(&store, "/none", &sources, "g1", 0, 10_000_000, 10).unwrap();
        let detail = g1.expect("g1 resolves");
        assert_eq!(detail.group.group_id, "g1");
        assert_eq!(detail.group.count, 1);
        assert_eq!(detail.group.trend.iter().sum::<i64>(), 1);
        assert_eq!(detail.variants.len(), 1);
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
    fn exception_detail_collapses_variants_and_attributes_releases() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        // One group, three occurrences: two share a message/stack (one variant
        // of count 2), the third differs. Different app versions throughout,
        // and the latest occurrence of the repeated variant carries metadata.
        let mut a1 = exc("g1", 1_000);
        a1.exc_message = Some("boom at start".into());
        a1.exc_stack = Some("at start (app.js)".into());
        a1.app_version = Some("1.0.0".into());
        let mut a2 = exc("g1", 2_000);
        a2.exc_message = Some("boom at start".into());
        a2.exc_stack = Some("at start (app.js)".into());
        a2.app_version = Some("1.1.0".into());
        a2.metadata_json = Some(r#"{"feature_flag":"checkout-v2"}"#.into());
        a2.sid = Some("sess-1".into());
        let mut b = exc("g1", 3_000);
        b.exc_message = Some("boom at shutdown".into());
        b.exc_stack = Some("at shutdown (app.js)".into());
        b.app_version = Some("1.1.0".into());
        store.append_events(&[a1, a2, b]).unwrap();

        let sources = ["https://a.com".to_string()];
        let detail = exception_detail(&store, "/none", &sources, "g1", 0, 10_000, 10)
            .unwrap()
            .expect("g1 resolves");

        // Two distinct variants; the repeated one carries its count and the
        // context (source-as-app, version, metadata) of its latest occurrence.
        assert_eq!(detail.variants.len(), 2);
        let repeated = detail
            .variants
            .iter()
            .find(|v| v.message == "boom at start")
            .unwrap();
        assert_eq!(repeated.count, 2);
        assert_eq!(repeated.source.as_deref(), Some("https://a.com"));
        assert_eq!(repeated.app_version.as_deref(), Some("1.1.0"));
        assert_eq!(
            repeated.metadata.as_deref(),
            Some(r#"{"feature_flag":"checkout-v2"}"#)
        );
        // The exemplar links to the session of its latest session-linked
        // occurrence.
        assert_eq!(repeated.session_id.as_deref(), Some("sess-1"));

        // The group's sessions surface as trace summaries for the picker.
        assert_eq!(detail.traces.len(), 1);
        assert_eq!(detail.traces[0].session_id, "sess-1");
        assert_eq!(detail.traces[0].exceptions, 1);

        // Distributions cover app releases (1.1.0 twice, 1.0.0 once). All the
        // occurrences share one source, so versions are keyed bare — the
        // application is given by the (source-scoped) view.
        let versions = &detail.breakdowns.app_versions;
        assert_eq!(
            versions.first().map(|r| (r.key.as_str(), r.count)),
            Some(("1.1.0", 2))
        );
        assert!(versions.iter().any(|r| r.key == "1.0.0" && r.count == 1));

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    /// A custom event belonging to a session.
    fn custom_in(source: &str, sid: &str, received_ms: i64, name: &str) -> StoredEvent {
        StoredEvent {
            created_ms: received_ms,
            received_ms,
            bid: "b".into(),
            sid: Some(sid.into()),
            kind: EventKind::Custom,
            source: source.into(),
            event_name: Some(name.into()),
            ..Default::default()
        }
    }

    #[test]
    fn dashboard_breaks_down_event_names() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let mut unnamed = custom_in("https://a.com", "s1", 4_000, "ignored");
        unnamed.event_name = None;
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                custom_in("https://a.com", "s1", 2_000, "signup"),
                custom_in("https://a.com", "s1", 3_000, "signup"),
                custom_in("https://a.com", "s2", 3_500, "checkout"),
                unnamed,
            ])
            .unwrap();

        let dash = dashboard(&store, "/none", None, 0, 10_000, 86_400_000).unwrap();
        let names: Vec<(&str, i64)> = dash
            .breakdowns
            .event_names
            .iter()
            .map(|r| (r.key.as_str(), r.events))
            .collect();
        // Ranked by count; unnamed events fold under the empty sentinel; page
        // loads contribute nothing.
        assert_eq!(names[0], ("signup", 2));
        assert!(names.contains(&("checkout", 1)));
        assert!(names.contains(&("", 1)));
        assert_eq!(names.len(), 3);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn event_detail_collapses_metadata_variants() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let mut with_meta = custom_in("https://a.com", "s1", 2_000, "signup");
        with_meta.metadata_json = Some(r#"{"plan":"pro"}"#.into());
        with_meta.pathname = Some("/pricing".into());
        let mut repeat = custom_in("https://a.com", "s2", 3_000, "signup");
        repeat.metadata_json = Some(r#"{"plan":"pro"}"#.into());
        repeat.pathname = Some("/pricing".into());
        repeat.ua_browser = Some("Firefox".into());
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                with_meta,
                repeat,
                custom_in("https://a.com", "s1", 4_000, "signup"),
                custom_in("https://a.com", "s1", 5_000, "checkout"),
            ])
            .unwrap();

        let detail = event_detail(&store, "/none", "signup", 0, 10_000, None, 10)
            .unwrap()
            .expect("signup resolves");
        assert_eq!(detail.name, "signup");
        assert_eq!(detail.count, 3);
        assert_eq!(detail.first_seen_ms, 2_000);
        assert_eq!(detail.last_seen_ms, 4_000);
        assert_eq!(detail.trend.iter().sum::<i64>(), 3);

        // Two variants: the shared metadata (count 2, context from its latest
        // occurrence) and the metadata-less one.
        assert_eq!(detail.variants.len(), 2);
        let repeated = detail
            .variants
            .iter()
            .find(|v| v.metadata.is_some())
            .unwrap();
        assert_eq!(repeated.count, 2);
        assert_eq!(repeated.metadata.as_deref(), Some(r#"{"plan":"pro"}"#));
        assert_eq!(repeated.ua_browser.as_deref(), Some("Firefox"));
        assert_eq!(repeated.pathname.as_deref(), Some("/pricing"));
        assert_eq!(repeated.session_id.as_deref(), Some("s2"));

        // Distributions cover the event's occurrences only.
        let pages = &detail.breakdowns.pages;
        assert!(pages.iter().any(|r| r.key == "/pricing" && r.count == 2));

        // Both sessions surface as traces; the other event name resolves
        // separately, and an unknown one not at all.
        assert_eq!(detail.traces.len(), 2);
        assert!(
            event_detail(&store, "/none", "checkout", 0, 10_000, None, 10)
                .unwrap()
                .is_some()
        );
        assert!(
            event_detail(&store, "/none", "nope", 0, 10_000, None, 10)
                .unwrap()
                .is_none()
        );

        // The dashboard filter scopes the detail like every other panel.
        let filter = dash_filter(&store, r#"source == "https://other.com""#);
        assert!(
            event_detail(&store, "/none", "signup", 0, 10_000, Some(&filter), 10)
                .unwrap()
                .is_none()
        );

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn exception_versions_qualify_only_across_sources() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let mut a = exc_on("https://a.com", "g1", 1_000);
        a.app_version = Some("1.0.0".into());
        let mut b = exc_on("https://b.com", "g1", 2_000);
        b.app_version = Some("1.0.0".into());
        store.append_events(&[a, b]).unwrap();

        // Across two sources the bare number would be ambiguous, so rows stay
        // qualified as `app @ version`.
        let sources = ["https://a.com".to_string(), "https://b.com".to_string()];
        let detail = exception_detail(&store, "/none", &sources, "g1", 0, 10_000, 10)
            .unwrap()
            .expect("g1 resolves");
        let keys: Vec<&str> = detail
            .breakdowns
            .app_versions
            .iter()
            .map(|r| r.key.as_str())
            .collect();
        assert!(keys.contains(&"a.com @ 1.0.0"));
        assert!(keys.contains(&"b.com @ 1.0.0"));

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn dashboard_samples_recent_session_traces() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        // Session s1: lands on /home, fires a custom event, then crashes.
        let mut s1_load = load("https://a.com", 1_000, true, None);
        s1_load.sid = Some("s1".into());
        let mut s1_exc = exc("g1", 3_000);
        s1_exc.sid = Some("s1".into());
        s1_exc.app_version = Some("1.2.0".into());
        // Session s2 starts later, on a different page.
        let mut s2_load = load("https://a.com", 5_000, false, None);
        s2_load.sid = Some("s2".into());
        s2_load.pathname = Some("/pricing".into());
        s2_load.ua_device = Some("Desktop".into());
        // A sessionless page view (pre-session tracker) forms no trace.
        let plain = load("https://a.com", 6_000, false, None);
        store
            .append_events(&[
                s1_load,
                custom_in("https://a.com", "s1", 2_000, "signup"),
                s1_exc,
                s2_load,
                plain,
            ])
            .unwrap();

        let dash = dashboard(&store, "/none", None, 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.traces.len(), 2);

        // Newest session first.
        assert_eq!(dash.traces[0].session_id, "s2");
        assert_eq!(dash.traces[0].entry_path.as_deref(), Some("/pricing"));
        assert_eq!(dash.traces[0].ua_device.as_deref(), Some("Desktop"));

        let s1 = &dash.traces[1];
        assert_eq!(s1.session_id, "s1");
        assert_eq!(s1.started_ms, 1_000);
        assert_eq!(s1.last_ms, 3_000);
        assert_eq!(s1.source, "https://a.com");
        assert_eq!(s1.entry_path.as_deref(), Some("/home"));
        assert_eq!(s1.ua_browser.as_deref(), Some("Chrome"));
        assert_eq!(s1.ua_version.as_deref(), Some("120.0"));
        assert_eq!(s1.app_version.as_deref(), Some("1.2.0"));
        assert_eq!(s1.pageviews, 1);
        assert_eq!(s1.events, 1);
        assert_eq!(s1.exceptions, 1);

        // The dashboard filter scopes traces like every other panel.
        let filter = dash_filter(&store, r#"path == "/pricing""#);
        let dash = dashboard(&store, "/none", Some(&filter), 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.traces.len(), 1);
        assert_eq!(dash.traces[0].session_id, "s2");

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn session_trace_returns_the_ordered_timeline() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        let mut s1_load = load("https://a.com", 1_000, true, None);
        s1_load.sid = Some("s1".into());
        let mut s1_exc = exc("g1", 3_000);
        s1_exc.sid = Some("s1".into());
        let mut other = load("https://a.com", 1_500, false, None);
        other.sid = Some("other".into());
        // Appended out of order: the timeline must come back sorted by time.
        store
            .append_events(&[
                s1_exc,
                s1_load,
                custom_in("https://a.com", "s1", 2_000, "signup"),
                other,
            ])
            .unwrap();

        let trace = session_trace(&store, "/none", "s1", 0, 10_000, 1_000)
            .unwrap()
            .expect("s1 resolves");
        assert_eq!(trace.session_id, "s1");
        assert_eq!(trace.source, "https://a.com");
        assert_eq!(trace.started_ms, 1_000);
        assert_eq!(trace.ended_ms, 3_000);
        assert_eq!(trace.ua_browser.as_deref(), Some("Chrome"));

        let kinds: Vec<TraceEventKind> = trace.events.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TraceEventKind::PageLoad,
                TraceEventKind::Custom,
                TraceEventKind::Exception,
            ]
        );
        assert_eq!(trace.events[0].pathname.as_deref(), Some("/home"));
        assert_eq!(trace.events[1].event_name.as_deref(), Some("signup"));
        assert_eq!(trace.events[2].exc_type.as_deref(), Some("TypeError"));
        assert_eq!(trace.events[2].exc_group.as_deref(), Some("g1"));

        // An unknown session resolves to None.
        assert!(
            session_trace(&store, "/none", "nope", 0, 10_000, 1_000)
                .unwrap()
                .is_none()
        );

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn timeseries_counts_exceptions_alongside_traffic() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                exc_on("https://a.com", "g1", 1_500),
                exc_on("https://a.com", "g1", 1_600),
            ])
            .unwrap();

        let dash = dashboard(&store, "/none", None, 0, 10_000, 86_400_000).unwrap();
        assert_eq!(dash.timeseries.len(), 1);
        assert_eq!(dash.timeseries[0].pageviews, 1);
        assert_eq!(dash.timeseries[0].exceptions, 2);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }

    #[test]
    fn dashboard_surfaces_pixel_and_custom_sources() {
        let redb = temp_redb();
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[
                load("https://a.com", 1_000, true, None),
                typed("pixel://p1", 2_000, EventKind::Pixel),
                typed("app://svc", 3_000, EventKind::Custom),
            ])
            .unwrap();

        let dash = dashboard(&store, "/none", None, 0, 10_000, 86_400_000).unwrap();
        let uris: Vec<&str> = dash.unassigned.iter().map(|u| u.key.as_str()).collect();
        assert!(uris.contains(&"https://a.com"));
        assert!(uris.contains(&"pixel://p1")); // previously invisible
        assert!(uris.contains(&"app://svc")); // previously invisible

        // The website keeps its visitor count; pixel/custom count as events, not
        // pageviews, in both the rollup and the headline summary.
        let site = dash
            .unassigned
            .iter()
            .find(|u| u.key == "https://a.com")
            .unwrap();
        assert_eq!(site.visitors, 1);
        assert_eq!(site.pageviews, 1);
        assert_eq!(site.events, 0);
        let pixel = dash
            .unassigned
            .iter()
            .find(|u| u.key == "pixel://p1")
            .unwrap();
        assert_eq!(pixel.events, 1);
        assert_eq!(dash.summary.pageviews, 1);
        assert_eq!(dash.summary.events, 2);

        drop(store);
        let _ = std::fs::remove_file(&redb);
    }
}
