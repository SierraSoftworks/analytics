//! Columnar bridge between [`StoredEvent`]s and Parquet partitions via polars.

use std::path::Path;
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use polars::prelude::*;

use super::event::{EventKind, StoredEvent};
use super::tables::STORAGE_ADVICE;
use crate::errors::{Result, ResultExt};

/// Guards the *layout* of the Parquet archive (one archive per process). Queries
/// hold the read side across list-files -> scan -> collect so a partition can
/// never be deleted out from under an in-flight scan; the compactor's
/// consolidation (and retention) hold the write side while replacing or removing
/// partition files. Plain additive writes need no lock — a query that listed the
/// directory a moment earlier simply doesn't see the new file yet.
static ARCHIVE_LOCK: RwLock<()> = RwLock::new(());

/// Take the archive layout lock for reading (see [`ARCHIVE_LOCK`]).
pub fn archive_read() -> RwLockReadGuard<'static, ()> {
    ARCHIVE_LOCK.read().unwrap_or_else(PoisonError::into_inner)
}

/// Take the archive layout lock for writing (see [`ARCHIVE_LOCK`]).
pub fn archive_write() -> RwLockWriteGuard<'static, ()> {
    ARCHIVE_LOCK.write().unwrap_or_else(PoisonError::into_inner)
}

/// Build a columnar [`DataFrame`] from a batch of events. Timestamps are kept as
/// `i64` epoch-millis columns; time bucketing is done with integer arithmetic at
/// query time, which avoids pulling in polars' temporal feature set for storage.
pub fn build_dataframe(events: &[StoredEvent]) -> PolarsResult<DataFrame> {
    let n = events.len();

    macro_rules! col {
        ($field:ident) => {{
            let mut v = Vec::with_capacity(n);
            for e in events {
                v.push(e.$field.clone());
            }
            v
        }};
    }

    let kind: Vec<String> = events.iter().map(|e| e.kind.as_str().to_string()).collect();

    df![
        "created_ms" => col!(created_ms),
        "received_ms" => col!(received_ms),
        "seq" => col!(seq),
        "bid" => col!(bid),
        "sid" => col!(sid),
        "kind" => kind,
        "source" => col!(source),
        "pathname" => col!(pathname),
        "is_unique_user" => col!(is_unique_user),
        "is_unique_page" => col!(is_unique_page),
        "referrer_host" => col!(referrer_host),
        "referrer_group" => col!(referrer_group),
        "country" => col!(country),
        "language" => col!(language),
        "ua_browser" => col!(ua_browser),
        "ua_version" => col!(ua_version),
        "ua_os" => col!(ua_os),
        "ua_device" => col!(ua_device),
        "utm_source" => col!(utm_source),
        "utm_medium" => col!(utm_medium),
        "utm_campaign" => col!(utm_campaign),
        "duration_ms" => col!(duration_ms),
        "event_name" => col!(event_name),
        "metadata_json" => col!(metadata_json),
        "app_version" => col!(app_version),
        "exc_type" => col!(exc_type),
        "exc_message" => col!(exc_message),
        "exc_stack" => col!(exc_stack),
        "exc_group" => col!(exc_group),
        "exc_handled" => col!(exc_handled),
    ]
}

/// Write a batch of events to a Parquet partition file, creating parent dirs.
/// The file is written to a `.tmp` sibling and atomically renamed into place, so a
/// concurrent reader never sees a half-written partition (and the `.tmp` extension
/// keeps any crash-orphaned temp out of the `*.parquet` scan).
pub fn write_partition(events: &[StoredEvent], path: &Path) -> Result<()> {
    let mut df = build_dataframe(events).or_system_err(STORAGE_ADVICE)?;
    write_dataframe(&mut df, path)
}

/// Atomically write `df` to `path` (`.tmp` sibling then rename), creating parent
/// directories as needed.
pub(super) fn write_dataframe(df: &mut DataFrame, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).or_system_err(STORAGE_ADVICE)?;
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{file_name}.tmp"));
    {
        let file = std::fs::File::create(&tmp).or_system_err(STORAGE_ADVICE)?;
        ParquetWriter::new(file)
            .finish(df)
            .or_system_err(STORAGE_ADVICE)?;
    }
    std::fs::rename(&tmp, path).or_system_err(STORAGE_ADVICE)?;
    Ok(())
}

/// Merge several Parquet partition files into one at `out`: the union of their
/// rows (diagonal, so files written before a column existed still combine),
/// de-duplicated by the globally unique per-event `seq` (a crash between a
/// merge's write and its delete leaves the same events in two files) and sorted
/// by time. Returns the merged row count. The caller deletes the inputs once
/// this returns — never before, since a read failure here must leave the day
/// untouched.
pub fn merge_partitions(files: &[std::path::PathBuf], out: &Path) -> Result<usize> {
    let mut frames = Vec::with_capacity(files.len());
    for file in files {
        frames.push(read_partition(file)?.lazy());
    }
    let merged = concat(
        frames,
        UnionArgs {
            to_supertypes: true,
            diagonal: true,
            ..Default::default()
        },
    )
    .or_system_err(STORAGE_ADVICE)?;

    let mut df = merged.collect().or_system_err(STORAGE_ADVICE)?;
    // De-duplicating by `seq` is only sound where `seq` is actually present; a
    // partition predating the column reads back as nulls, which a subset-unique
    // would collapse into a single row. Such files never contain crash
    // duplicates anyway (the stamp predates the dedup design), so skip.
    let has_null_seq = df.column("seq").map(|c| c.null_count() > 0).unwrap_or(true);
    if !has_null_seq {
        df = df
            .lazy()
            .unique(
                Some(Selector::ByName {
                    names: [PlSmallStr::from("seq")].into(),
                    strict: true,
                }),
                UniqueKeepStrategy::Any,
            )
            .sort(["received_ms", "seq"], SortMultipleOptions::default())
            .collect()
            .or_system_err(STORAGE_ADVICE)?;
    }
    let rows = df.height();
    write_dataframe(&mut df, out)?;
    Ok(rows)
}

/// Read a Parquet partition back into a [`DataFrame`] (used by tests and ad-hoc
/// queries).
pub fn read_partition(path: &Path) -> Result<DataFrame> {
    let file = std::fs::File::open(path).or_system_err(STORAGE_ADVICE)?;
    ParquetReader::new(file)
        .finish()
        .or_system_err(STORAGE_ADVICE)
}

/// Recompute `exc_group` for the exception rows of the partition at `path`, using
/// `remap(exc_type, exc_message, exc_stack)`. The file is rewritten (atomically)
/// only when at least one group actually changes; returns the number of changed
/// occurrences.
pub(super) fn regroup_partition(
    path: &Path,
    remap: &dyn Fn(&str, Option<&str>, Option<&str>) -> String,
) -> Result<usize> {
    let mut df = read_partition(path)?;
    let height = df.height();
    if height == 0 {
        return Ok(0);
    }

    // Cast the columns we touch to String up front: an all-null column can come
    // back from Parquet as a Null dtype, which the typed `.str()` accessor rejects.
    let as_str = |df: &DataFrame, name: &str| -> Result<Column> {
        df.column(name)
            .and_then(|c| c.cast(&DataType::String))
            .or_system_err(STORAGE_ADVICE)
    };
    let kind = as_str(&df, "kind")?;
    let exc_type = as_str(&df, "exc_type")?;
    let exc_message = as_str(&df, "exc_message")?;
    let exc_stack = as_str(&df, "exc_stack")?;
    let exc_group = as_str(&df, "exc_group")?;

    let (kind, exc_type, exc_message, exc_stack, exc_group) = (
        kind.str().or_system_err(STORAGE_ADVICE)?,
        exc_type.str().or_system_err(STORAGE_ADVICE)?,
        exc_message.str().or_system_err(STORAGE_ADVICE)?,
        exc_stack.str().or_system_err(STORAGE_ADVICE)?,
        exc_group.str().or_system_err(STORAGE_ADVICE)?,
    );

    let mut new_groups: Vec<Option<String>> = Vec::with_capacity(height);
    let mut changed = 0usize;
    for i in 0..height {
        if kind.get(i) == Some(EventKind::Exception.as_str()) {
            let group = remap(
                exc_type.get(i).unwrap_or(""),
                exc_message.get(i),
                exc_stack.get(i),
            );
            if exc_group.get(i) != Some(group.as_str()) {
                changed += 1;
            }
            new_groups.push(Some(group));
        } else {
            new_groups.push(exc_group.get(i).map(str::to_string));
        }
    }

    if changed == 0 {
        return Ok(0);
    }

    df.with_column(Series::new("exc_group".into(), new_groups).into_column())
        .or_system_err(STORAGE_ADVICE)?;
    write_dataframe(&mut df, path)?;
    Ok(changed)
}
