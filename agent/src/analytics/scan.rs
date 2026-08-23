//! Streamed access to the event store: the hot∪cold union is visited one
//! partition frame at a time, so a query's peak memory is a single partition
//! plus the caller's aggregate state — never the whole window. (Materializing a
//! multi-week window through polars costs 5–10× the data size in decode/collect
//! transients; folding per partition measured ~24MB peak against the production
//! archive where whole-window collects cost 200–500MB.)
//!
//! Partition frames are served through the process-wide
//! [`partition_cache`](crate::store::partition_cache): decoded once, shared as
//! `Arc<DataFrame>`s by every concurrent query, and evicted LRU under a byte
//! budget — so a warm query touches no disk and allocates only its own
//! aggregate state.
//!
//! The archive layout is two-tier (see the compactor): sealed months hold one
//! `month-*.parquet` directly under `YYYY/MM/`, while the months still inside
//! the hot window's reach hold one file per `YYYY/MM/DD/` day directory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate, TimeZone, Utc};
use polars::prelude::*;
use tracing_batteries::prelude::warn;

use crate::errors::{Result, ResultExt};
use crate::store::{Store, partition_cache};

const ADVICE: &[&str] = &["This is an internal analytics error; please report it with the logs."];

/// Typed views over one partition frame's columns, canonicalized so folds can
/// index rows without caring which columns a (possibly older) partition file
/// actually carried: absent columns read as all-null, and all-null columns that
/// Parquet hands back as the `Null` dtype are cast to their canonical type.
pub(super) struct Rows {
    pub height: usize,
    pub received_ms: Int64Chunked,
    pub seq: UInt64Chunked,
    pub bid: StringChunked,
    pub sid: StringChunked,
    pub kind: StringChunked,
    pub source: StringChunked,
    pub pathname: StringChunked,
    pub is_unique_user: BooleanChunked,
    pub is_unique_page: BooleanChunked,
    pub referrer_host: StringChunked,
    pub country: StringChunked,
    pub language: StringChunked,
    pub ua_browser: StringChunked,
    pub ua_version: StringChunked,
    pub ua_os: StringChunked,
    pub ua_device: StringChunked,
    pub utm_source: StringChunked,
    pub utm_medium: StringChunked,
    pub utm_campaign: StringChunked,
    pub duration_ms: Int64Chunked,
    pub event_name: StringChunked,
    pub metadata_json: StringChunked,
    pub app_version: StringChunked,
    pub exc_type: StringChunked,
    pub exc_message: StringChunked,
    pub exc_stack: StringChunked,
    pub exc_group: StringChunked,
    pub exc_handled: BooleanChunked,
}

impl Rows {
    fn of(df: &DataFrame) -> Result<Rows> {
        fn cast<T: PolarsDataType>(
            df: &DataFrame,
            name: &str,
            dtype: &DataType,
            get: impl Fn(&Column) -> PolarsResult<&ChunkedArray<T>>,
        ) -> Result<ChunkedArray<T>>
        where
            ChunkedArray<T>: ChunkFullNull,
        {
            match df.column(name) {
                Ok(column) => {
                    let column = column.cast(dtype).or_system_err(ADVICE)?;
                    Ok(get(&column).or_system_err(ADVICE)?.clone())
                }
                Err(_) => Ok(ChunkedArray::full_null(name.into(), df.height())),
            }
        }
        let s = |name: &str| cast(df, name, &DataType::String, |c| c.str());
        let b = |name: &str| cast(df, name, &DataType::Boolean, |c| c.bool());
        Ok(Rows {
            height: df.height(),
            received_ms: cast(df, "received_ms", &DataType::Int64, |c| c.i64())?,
            seq: cast(df, "seq", &DataType::UInt64, |c| c.u64())?,
            duration_ms: cast(df, "duration_ms", &DataType::Int64, |c| c.i64())?,
            is_unique_user: b("is_unique_user")?,
            is_unique_page: b("is_unique_page")?,
            exc_handled: b("exc_handled")?,
            bid: s("bid")?,
            sid: s("sid")?,
            kind: s("kind")?,
            source: s("source")?,
            pathname: s("pathname")?,
            referrer_host: s("referrer_host")?,
            country: s("country")?,
            language: s("language")?,
            ua_browser: s("ua_browser")?,
            ua_version: s("ua_version")?,
            ua_os: s("ua_os")?,
            ua_device: s("ua_device")?,
            utm_source: s("utm_source")?,
            utm_medium: s("utm_medium")?,
            utm_campaign: s("utm_campaign")?,
            event_name: s("event_name")?,
            metadata_json: s("metadata_json")?,
            app_version: s("app_version")?,
            exc_type: s("exc_type")?,
            exc_message: s("exc_message")?,
            exc_stack: s("exc_stack")?,
            exc_group: s("exc_group")?,
        })
    }
}

/// The canonical column set, mirroring [`Rows`]. Cached frames are normalized
/// to this shape once at cache fill (see [`canonicalize`]), so building a
/// [`Rows`] view over one is pure reference-count traffic: every cast hits the
/// same-dtype fast path and no per-query null columns are materialized.
const COLUMNS: &[(&str, DataType)] = &[
    ("received_ms", DataType::Int64),
    ("seq", DataType::UInt64),
    ("duration_ms", DataType::Int64),
    ("is_unique_user", DataType::Boolean),
    ("is_unique_page", DataType::Boolean),
    ("exc_handled", DataType::Boolean),
    ("bid", DataType::String),
    ("sid", DataType::String),
    ("kind", DataType::String),
    ("source", DataType::String),
    ("pathname", DataType::String),
    ("referrer_host", DataType::String),
    ("country", DataType::String),
    ("language", DataType::String),
    ("ua_browser", DataType::String),
    ("ua_version", DataType::String),
    ("ua_os", DataType::String),
    ("ua_device", DataType::String),
    ("utm_source", DataType::String),
    ("utm_medium", DataType::String),
    ("utm_campaign", DataType::String),
    ("event_name", DataType::String),
    ("metadata_json", DataType::String),
    ("app_version", DataType::String),
    ("exc_type", DataType::String),
    ("exc_message", DataType::String),
    ("exc_stack", DataType::String),
    ("exc_group", DataType::String),
];

/// Normalize a freshly decoded partition frame: columns the file predates are
/// added as all-null, and all-null columns that Parquet hands back as the
/// `Null` dtype are cast to their canonical type. Runs once per cache fill.
fn canonicalize(mut df: DataFrame) -> Result<DataFrame> {
    let height = df.height();
    for (name, dtype) in COLUMNS {
        let column = match df.column(name) {
            Ok(column) if column.dtype() == dtype => continue,
            Ok(column) => column.cast(dtype).or_system_err(ADVICE)?,
            Err(_) => Column::full_null((*name).into(), height, dtype),
        };
        df.with_column(column).or_system_err(ADVICE)?;
    }
    Ok(df)
}

/// One partition file plus the UTC day span its directory pins it to: a single
/// day for `YYYY/MM/DD/` files, the whole month for a sealed `YYYY/MM/` file.
struct PartitionFile {
    path: PathBuf,
    first_day: (i32, u32, u32),
    last_day: (i32, u32, u32),
}

/// Stream every event in `[from_ms, to_ms)` matching `predicate` (the compiled
/// dashboard `q`, if any) through `visit`, one partition frame at a time: the
/// cold partitions in date order, then the hot store. Frames come from the
/// partition cache, so the common fully-covered/unfiltered case visits the
/// shared decoded frame with no copying at all.
///
/// Crash duplicates are removed by the globally unique per-event `seq` — a
/// Parquet row whose seq is still present in redb (a compaction that archived
/// but never deleted), or a repeat within a multi-file month (a consolidation
/// or seal that wrote its merged file but never removed the inputs) is
/// dropped, so every event is visited exactly once. An unreadable partition is
/// skipped with a warning: it must surface in the logs, not fail every query.
pub(super) fn scan_events(
    store: &Store,
    parquet_dir: &str,
    from_ms: i64,
    to_ms: i64,
    predicate: Option<Expr>,
    visit: &mut dyn FnMut(&Rows) -> Result<()>,
) -> Result<()> {
    let misses_before = partition_cache().misses();
    let hot = store.all_events()?;
    // Only partitions reaching the hot store's earliest day onward can hold a
    // copy of a hot event; older partitions need no membership checks.
    let hot_min_day = hot.iter().map(|e| e.received_ms).min().map(day_of);
    let hot_seqs: HashSet<u64> = hot.iter().map(|e| e.seq).collect();

    for files in partition_months(Path::new(parquet_dir), from_ms, to_ms) {
        // De-duplication scope is the month: a crashed seal or consolidation
        // leaves the same events in a merged file and its not-yet-deleted
        // inputs, which always share the month directory.
        let multi_file = files.len() > 1;
        let mut month_seen: HashSet<u64> = HashSet::new();
        for file in files {
            let overlaps_hot = hot_min_day.is_some_and(|min| file.last_day >= min);
            let frame = match partition_cache().get_with(&file.path, canonicalize) {
                Ok(frame) => frame,
                Err(err) => {
                    warn!(
                        "skipping unreadable parquet partition {}: {err}",
                        file.path.display()
                    );
                    continue;
                }
            };

            let covered = days_within(file.first_day, file.last_day, from_ms, to_ms);
            if covered && predicate.is_none() && !overlaps_hot && !multi_file {
                // The common case: the cached frame is visited as-is, shared
                // across every concurrent query.
                let rows = Rows::of(&frame)?;
                if rows.height > 0 {
                    visit(&rows)?;
                }
                continue;
            }

            let mut df = in_range((*frame).clone(), from_ms, to_ms, predicate.clone(), covered)?;
            if overlaps_hot || multi_file {
                df = drop_duplicates(df, overlaps_hot.then_some(&hot_seqs), &mut month_seen)?;
            }
            let rows = Rows::of(&df)?;
            if rows.height > 0 {
                visit(&rows)?;
            }
        }
    }

    let hot_df = crate::store::build_dataframe(&hot).or_system_err(ADVICE)?;
    let rows = Rows::of(&in_range(hot_df, from_ms, to_ms, predicate, false)?)?;
    if rows.height > 0 {
        visit(&rows)?;
    }

    // A scan that filled cache entries decoded fresh partitions; hand those
    // transient buffers back to the OS so RSS tracks live data.
    if partition_cache().misses() != misses_before {
        crate::store::trim_allocator();
    }
    Ok(())
}

/// Whether the file's whole day span lies inside `[from_ms, to_ms)`, in which
/// case the range filter is a no-op for its partition.
fn days_within(first: (i32, u32, u32), last: (i32, u32, u32), from_ms: i64, to_ms: i64) -> bool {
    let (Some(start), Some(end)) = (day_start_ms(first), day_start_ms(last)) else {
        return false;
    };
    start >= from_ms && end + 86_400_000 <= to_ms
}

/// The UTC-midnight instant of a `(year, month, day)`, if it is a valid date.
fn day_start_ms((year, month, day): (i32, u32, u32)) -> Option<i64> {
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
        .single()
        .map(|dt| dt.timestamp_millis())
}

/// Filter one partition frame to the half-open `[from, to)` window and the
/// compiled `q` predicate, if any. `covered` marks a frame whose whole span is
/// inside the window, which skips the (then tautological) range comparison.
fn in_range(
    df: DataFrame,
    from_ms: i64,
    to_ms: i64,
    predicate: Option<Expr>,
    covered: bool,
) -> Result<DataFrame> {
    let range = (!covered).then(|| {
        col("received_ms")
            .gt_eq(lit(from_ms))
            .and(col("received_ms").lt(lit(to_ms)))
    });
    let filter = match (range, predicate) {
        (Some(range), Some(predicate)) => range.and(predicate),
        (Some(range), None) => range,
        (None, Some(predicate)) => predicate,
        (None, None) => return Ok(df),
    };
    df.lazy().filter(filter).collect().or_system_err(ADVICE)
}

/// Drop rows whose `seq` was already seen — in the hot store (`hot_seqs`) or
/// earlier in the same month (`month_seen`). Rows with a null seq (partitions
/// predating the column) are never treated as duplicates.
fn drop_duplicates(
    df: DataFrame,
    hot_seqs: Option<&HashSet<u64>>,
    month_seen: &mut HashSet<u64>,
) -> Result<DataFrame> {
    let seq = df
        .column("seq")
        .and_then(|c| c.cast(&DataType::UInt64))
        .or_system_err(ADVICE)?;
    let seq = seq.u64().or_system_err(ADVICE)?;
    let keep: Vec<bool> = (0..df.height())
        .map(|i| match seq.get(i) {
            None => true,
            Some(s) => !hot_seqs.is_some_and(|hot| hot.contains(&s)) && month_seen.insert(s),
        })
        .collect();
    df.filter(&BooleanChunked::from_slice("keep".into(), &keep))
        .or_system_err(ADVICE)
}

/// Partition files grouped per month, months ascending, covering only months
/// that overlap `[from_ms, to_ms]` (and, within an unsealed month, only the
/// overlapping days) — pruning whole directories keeps a narrow query from
/// touching the entire archive. Sealed `month-*` files come first in a group,
/// then day files in day order (`day-*` ahead of leftover `events-*` inputs),
/// so a merged copy always wins de-duplication over its stragglers.
fn partition_months(dir: &Path, from_ms: i64, to_ms: i64) -> Vec<Vec<PartitionFile>> {
    let (from, to) = (day_of(from_ms), day_of(to_ms));
    let mut months: Vec<((i32, u32), Vec<PartitionFile>)> = Vec::new();
    for year in numeric_subdirs::<i32>(dir) {
        let year_dir = dir.join(format!("{year:04}"));
        for month in numeric_subdirs::<u32>(&year_dir) {
            if (year, month) < (from.0, from.1) || (year, month) > (to.0, to.1) {
                continue;
            }
            let month_dir = year_dir.join(format!("{month:02}"));
            let mut group: Vec<PartitionFile> = Vec::new();

            // Sealed month files sit directly under YYYY/MM/.
            let (first_day, last_day) = month_days(year, month);
            let mut sealed = parquet_files_in(&month_dir);
            sealed.sort();
            group.extend(sealed.into_iter().map(|path| PartitionFile {
                path,
                first_day,
                last_day,
            }));

            // Day files for the (typically current) unsealed layout.
            let mut days = numeric_subdirs::<u32>(&month_dir);
            days.sort_unstable();
            for day in days {
                let date = (year, month, day);
                if date < from || date > to {
                    continue;
                }
                let mut files = parquet_files_in(&month_dir.join(format!("{day:02}")));
                files.sort();
                group.extend(files.into_iter().map(|path| PartitionFile {
                    path,
                    first_day: date,
                    last_day: date,
                }));
            }

            if !group.is_empty() {
                months.push(((year, month), group));
            }
        }
    }
    months.sort_by_key(|(month, _)| *month);
    months.into_iter().map(|(_, group)| group).collect()
}

/// The first and last `(year, month, day)` of a month.
fn month_days(year: i32, month: u32) -> ((i32, u32, u32), (i32, u32, u32)) {
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    let last = next
        .and_then(|d| d.pred_opt())
        .map(|d| d.day())
        .unwrap_or(31);
    ((year, month, 1), (year, month, last))
}

/// The `*.parquet` files directly inside `dir` (not recursive).
fn parquet_files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "parquet"))
        .collect()
}

/// The UTC-midnight instant of the earliest partition in the archive, if any:
/// the earliest `YYYY/MM/DD` day directory, or the first of the earliest
/// `YYYY/MM` month holding a sealed month file.
pub(super) fn earliest_partition_ms(dir: &Path) -> Option<i64> {
    let year = numeric_subdirs::<i32>(dir).into_iter().min()?;
    let year_dir = dir.join(format!("{year:04}"));
    let month = numeric_subdirs::<u32>(&year_dir).into_iter().min()?;
    let month_dir = year_dir.join(format!("{month:02}"));
    let day = if parquet_files_in(&month_dir).is_empty() {
        numeric_subdirs::<u32>(&month_dir).into_iter().min()?
    } else {
        // A sealed month file may hold events as early as the first of the
        // month.
        1
    };
    day_start_ms((year, month, day))
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
