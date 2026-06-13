//! Periodically seal the redb hot window into date-partitioned Parquet files and
//! enforce retention. Reads-then-writes-then-deletes so a write failure never
//! loses data.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, TimeZone, Utc};
use tokio::time::MissedTickBehavior;
use tracing_batteries::prelude::*;

use crate::config::StorageConfig;
use crate::errors::Result;
use crate::store::{Store, StoredEvent, write_partition};

pub(super) async fn run(store: Arc<Store>, storage: StorageConfig) {
    // Honour the configured interval; floor at 1s only to avoid a busy loop if it is
    // misconfigured to zero.
    let interval = storage.rollup_interval.max(Duration::from_secs(1));
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tick.tick().await;
        let store = store.clone();
        let storage = storage.clone();
        match tokio::task::spawn_blocking(move || compact_once(&store, &storage)).await {
            Ok(Ok(0)) => {}
            Ok(Ok(n)) => info!("compacted {n} events to Parquet"),
            Ok(Err(err)) => error!("compaction failed: {err}"),
            Err(err) => error!("compactor task panicked: {err}"),
        }
    }
}

fn compact_once(store: &Store, storage: &StorageConfig) -> Result<usize> {
    let now = Utc::now().timestamp_millis();
    let cutoff = now - storage.hot_window.as_millis() as i64;
    let written = compact_window(store, Path::new(&storage.parquet_dir), cutoff, now)?;
    enforce_retention(storage);
    Ok(written)
}

/// Seal every event older than `cutoff_ms` into date-partitioned Parquet, then drop
/// it from redb. Reads then writes then deletes, so a write failure loses nothing.
/// `stamp` disambiguates partition filenames within a run.
fn compact_window(store: &Store, parquet_dir: &Path, cutoff_ms: i64, stamp: i64) -> Result<usize> {
    let events = store.events_before(cutoff_ms)?;
    if events.is_empty() {
        return Ok(0);
    }

    // Group by UTC date so each partition holds one day's events.
    let mut by_date: BTreeMap<(i32, u32, u32), Vec<StoredEvent>> = BTreeMap::new();
    for event in events {
        let date = Utc
            .timestamp_millis_opt(event.received_ms)
            .single()
            .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap());
        by_date
            .entry((date.year(), date.month(), date.day()))
            .or_default()
            .push(event);
    }

    let mut total = 0;
    for ((year, month, day), group) in by_date {
        let file = parquet_dir
            .join(format!("{year:04}"))
            .join(format!("{month:02}"))
            .join(format!("{day:02}"))
            .join(format!("events-{stamp}.parquet"));
        write_partition(&group, &file)?;
        total += group.len();
    }

    // Only drop the window once every partition is safely written.
    store.delete_before(cutoff_ms)?;
    Ok(total)
}

/// Best-effort deletion of day partitions older than the retention window.
fn enforce_retention(storage: &StorageConfig) {
    let root = Path::new(&storage.parquet_dir);
    if !root.exists() {
        return;
    }
    let retention = chrono::Duration::from_std(storage.retention)
        .unwrap_or_else(|_| chrono::Duration::days(365));
    let cutoff = Utc::now() - retention;

    for year in dir_numbers(root) {
        let year_dir = root.join(format!("{year:04}"));
        for month in dir_numbers(&year_dir) {
            let month_dir = year_dir.join(format!("{month:02}"));
            for day in dir_numbers(&month_dir) {
                let Some(date) = chrono::NaiveDate::from_ymd_opt(year, month, day) else {
                    continue;
                };
                let end_of_day = date.and_hms_opt(23, 59, 59).unwrap();
                if Utc.from_utc_datetime(&end_of_day) < cutoff {
                    let _ = std::fs::remove_dir_all(month_dir.join(format!("{day:02}")));
                }
            }
        }
    }
}

/// Numeric subdirectory names (year/month/day) under `dir`.
fn dir_numbers<T: std::str::FromStr>(dir: &Path) -> Vec<T> {
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
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::store::{EventKind, Store, StoredEvent};

    fn temp(suffix: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("analytics-compact-{}-{}-{}", std::process::id(), n, suffix))
    }

    fn event(received_ms: i64) -> StoredEvent {
        StoredEvent {
            received_ms,
            created_ms: received_ms,
            bid: "b".into(),
            kind: EventKind::PageLoad,
            source: "https://example.com".into(),
            ..Default::default()
        }
    }

    #[test]
    fn compacts_old_events_to_parquet_and_clears_redb() {
        let redb = temp("redb");
        let parquet = temp("parquet");
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[event(1_000), event(2_000), event(9_999_999_999_999)])
            .unwrap();

        // Cutoff excludes the far-future event.
        let written = compact_window(&store, &parquet, 5_000, 42).unwrap();
        assert_eq!(written, 2);
        assert_eq!(store.event_count().unwrap(), 1);

        let files: Vec<_> = walk(&parquet);
        assert_eq!(files.len(), 1, "one daily partition written");
        assert!(files[0].ends_with("events-42.parquet"));

        // Nothing left to compact at the same cutoff.
        assert_eq!(compact_window(&store, &parquet, 5_000, 43).unwrap(), 0);

        drop(store);
        let _ = std::fs::remove_file(&redb);
        let _ = std::fs::remove_dir_all(&parquet);
    }

    fn walk(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    out.extend(walk(&path));
                } else if path.extension().is_some_and(|e| e == "parquet") {
                    out.push(path.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        out
    }
}
