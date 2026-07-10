//! Columnar bridge between [`StoredEvent`]s and Parquet partitions via polars.

use std::path::Path;

use polars::prelude::*;

use super::event::{EventKind, StoredEvent};
use super::tables::STORAGE_ADVICE;
use crate::errors::{Result, ResultExt};

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
            let group = remap(exc_type.get(i).unwrap_or(""), exc_message.get(i), exc_stack.get(i));
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
