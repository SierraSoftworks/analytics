//! Drains the redb WAL into the Parquet archive and maintains the archive's
//! two-tier layout.
//!
//! redb plays the write-ahead log: events land there at ingest and stay until
//! they age past the hot window (late-arriving events for the same period may
//! still be in flight before that). Each tick then:
//!
//! 1. **Compacts** every WAL entry older than the hot-window cutoff into its
//!    `YYYY/MM/DD/` day partition (read-keys → write-Parquet → delete-keys, so
//!    a write failure never loses data; a crash in between leaves a window in
//!    both stores, which queries de-duplicate by the per-event `seq`).
//! 2. **Consolidates** any day directory holding more than one file into a
//!    single sorted partition (the hourly tick would otherwise leave ~24 tiny
//!    files per day, and per-file overhead — not data volume — is what makes
//!    wide queries expensive).
//! 3. **Seals** every month that the WAL can no longer write into (its last
//!    day is behind the hot-window cutoff) into one immutable
//!    `YYYY/MM/month-*.parquet`, deleting the day layout beneath it. Parquet
//!    files cannot be appended to, so "append-only monthly partitions" means
//!    exactly this: days accumulate while a month is open, and the month is
//!    rewritten once — at seal — after which it never changes again.
//! 4. **Enforces retention**: a sealed month is removed whole once its *last*
//!    day falls out of the retention window (month-granular truncation);
//!    still-open day layouts are trimmed day by day.
//!
//! Every merge/delete holds the archive layout lock and invalidates the
//! partition cache for the files it removes, so in-flight queries never lose a
//! partition mid-scan and never serve stale frames afterwards.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, TimeZone, Utc};
use tokio::time::MissedTickBehavior;
use tracing_batteries::prelude::*;

use crate::config::StorageConfig;
use crate::errors::Result;
use crate::store::{
    Store, StoredEvent, archive_write, merge_partitions, partition_cache, write_partition,
};

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
    // Consolidation and sealing are optimizations: a failure (e.g. an
    // unreadable file) must not stop compaction or retention.
    match consolidate(Path::new(&storage.parquet_dir), now) {
        Ok(0) => {}
        Ok(n) => info!("consolidated {n} parquet partition files into daily partitions"),
        Err(err) => warn!("partition consolidation failed: {err}"),
    }
    match seal_months(Path::new(&storage.parquet_dir), cutoff, now) {
        Ok(0) => {}
        Ok(n) => info!("sealed {n} parquet partition files into monthly partitions"),
        Err(err) => warn!("partition sealing failed: {err}"),
    }
    enforce_retention(storage);
    partition_cache().prune_missing();
    // Merges decode and rewrite whole partitions; return those transient
    // buffers to the OS rather than letting the allocator retain them.
    crate::store::trim_allocator();
    Ok(written)
}

/// Seal every event older than `cutoff_ms` into date-partitioned Parquet, then drop
/// exactly those keys from redb. Read-keys -> write-Parquet -> delete-those-keys, so
/// an event committed after the read is never deleted without being archived; and if
/// a crash leaves a window in both stores, the per-event `seq` lets queries
/// de-duplicate it. `stamp` disambiguates partition filenames within a run.
fn compact_window(store: &Store, parquet_dir: &Path, cutoff_ms: i64, stamp: i64) -> Result<usize> {
    let pairs = store.events_before_with_keys(cutoff_ms)?;
    if pairs.is_empty() {
        return Ok(0);
    }

    // Group by UTC date so each partition holds one day's events; remember the exact
    // keys to delete once they are safely archived.
    let mut keys: Vec<Vec<u8>> = Vec::with_capacity(pairs.len());
    let mut by_date: BTreeMap<(i32, u32, u32), Vec<StoredEvent>> = BTreeMap::new();
    for (key, event) in pairs {
        keys.push(key);
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

    store.delete_keys(&keys)?;
    Ok(total)
}

/// Merge every day directory holding more than one partition file into a single
/// sorted, de-duplicated file named `day-<stamp>.parquet` (a name the same tick's
/// `compact_window` output can never collide with). Returns the number of input
/// files that were merged away. The write happens before the inputs are deleted,
/// so a crash in between leaves duplicate events on disk — which queries and the
/// next merge both collapse via the per-event `seq`.
pub(crate) fn consolidate(parquet_dir: &Path, stamp: i64) -> Result<usize> {
    let mut merged = 0;
    for year in dir_numbers::<i32>(parquet_dir) {
        let year_dir = parquet_dir.join(format!("{year:04}"));
        for month in dir_numbers::<u32>(&year_dir) {
            let month_dir = year_dir.join(format!("{month:02}"));
            for day in dir_numbers::<u32>(&month_dir) {
                let day_dir = month_dir.join(format!("{day:02}"));
                let mut files = parquet_files_in(&day_dir);
                if files.len() < 2 {
                    continue;
                }
                files.sort();

                // Hold the layout lock across replace-and-delete so an in-flight
                // query never sees a partition vanish mid-scan.
                let _layout = archive_write();
                merge_partitions(&files, &day_dir.join(format!("day-{stamp}.parquet")))?;
                remove_merged(&files);
                merged += files.len();
            }
        }
    }
    Ok(merged)
}

/// Seal every month whose last day is behind the WAL cutoff — no future
/// compaction can write into it — by merging its day partitions (and any
/// earlier sealed file a crashed run left behind) into a single immutable
/// `month-<stamp>.parquet` directly under `YYYY/MM/`, then removing the day
/// layout. Idempotent and crash-safe like consolidation: the merged file is
/// written before its inputs are deleted, and duplicates collapse by `seq`.
/// Returns the number of input files merged away.
pub(crate) fn seal_months(parquet_dir: &Path, cutoff_ms: i64, stamp: i64) -> Result<usize> {
    let cutoff = Utc
        .timestamp_millis_opt(cutoff_ms)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap());
    let cutoff_month = (cutoff.year(), cutoff.month());

    let mut sealed = 0;
    for year in dir_numbers::<i32>(parquet_dir) {
        let year_dir = parquet_dir.join(format!("{year:04}"));
        for month in dir_numbers::<u32>(&year_dir) {
            // Only months strictly before the cutoff's month are final; the
            // cutoff month itself can still receive compaction writes.
            if (year, month) >= cutoff_month {
                continue;
            }
            let month_dir = year_dir.join(format!("{month:02}"));
            let month_files = parquet_files_in(&month_dir);
            let day_dirs = dir_numbers::<u32>(&month_dir);
            if day_dirs.is_empty() && month_files.len() <= 1 {
                continue; // already sealed (or nothing to seal)
            }

            let mut inputs = month_files;
            for day in &day_dirs {
                inputs.extend(parquet_files_in(&month_dir.join(format!("{day:02}"))));
            }
            inputs.sort();
            if !inputs.is_empty() {
                let _layout = archive_write();
                merge_partitions(&inputs, &month_dir.join(format!("month-{stamp}.parquet")))?;
                remove_merged(&inputs);
                sealed += inputs.len();
            }
            for day in day_dirs {
                // Now-empty day directories; remove_dir refuses non-empty ones.
                let _ = std::fs::remove_dir(month_dir.join(format!("{day:02}")));
            }
        }
    }
    Ok(sealed)
}

/// Delete merged-away partition files and drop them from the partition cache.
fn remove_merged(files: &[PathBuf]) {
    for file in files {
        if let Err(err) = std::fs::remove_file(file) {
            warn!(
                "failed to remove merged partition {}: {err}",
                file.display()
            );
        }
        partition_cache().invalidate(file);
    }
}

/// Best-effort deletion of expired partitions: a month is removed whole once
/// its last day is older than the retention window (the sealed layout has no
/// finer grain — this is the trade for month files); a still-open month's day
/// directories are trimmed day by day.
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
        for month in dir_numbers::<u32>(&year_dir) {
            let month_dir = year_dir.join(format!("{month:02}"));
            let next_month = if month == 12 {
                chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1)
            } else {
                chrono::NaiveDate::from_ymd_opt(year, month + 1, 1)
            };
            let month_expired = next_month
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .is_some_and(|end| Utc.from_utc_datetime(&end) < cutoff);
            if month_expired {
                let _layout = archive_write();
                let _ = std::fs::remove_dir_all(&month_dir);
                continue;
            }
            for day in dir_numbers(&month_dir) {
                let Some(date) = chrono::NaiveDate::from_ymd_opt(year, month, day) else {
                    continue;
                };
                let end_of_day = date.and_hms_opt(23, 59, 59).unwrap();
                if Utc.from_utc_datetime(&end_of_day) < cutoff {
                    let _layout = archive_write();
                    let _ = std::fs::remove_dir_all(month_dir.join(format!("{day:02}")));
                }
            }
        }
    }
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
        std::env::temp_dir().join(format!(
            "analytics-compact-{}-{}-{}",
            std::process::id(),
            n,
            suffix
        ))
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

    #[test]
    fn consolidates_fragmented_days_into_one_partition() {
        let redb = temp("redb");
        let parquet = temp("parquet");
        let store = Store::open(&redb).unwrap();

        // Three hourly ticks worth of events on one day, plus one on another day.
        store
            .append_events(&[event(1_000), event(3_600_000), event(7_200_000)])
            .unwrap();
        assert_eq!(compact_window(&store, &parquet, 3_000, 1).unwrap(), 1);
        assert_eq!(compact_window(&store, &parquet, 3_700_000, 2).unwrap(), 1);
        assert_eq!(
            compact_window(&store, &parquet, 99_999_999_999, 3).unwrap(),
            1
        );
        store.append_events(&[event(90_000_000_000)]).unwrap(); // a later day
        assert_eq!(
            compact_window(&store, &parquet, 99_999_999_999_999, 4).unwrap(),
            1
        );
        assert_eq!(
            walk(&parquet).len(),
            4,
            "three files day one, one file day two"
        );

        // Consolidation merges day one's three files into a single partition and
        // leaves the already-dense day alone.
        assert_eq!(consolidate(&parquet, 5).unwrap(), 3);
        let files = walk(&parquet);
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.ends_with("day-5.parquet")));

        // Every event survived, exactly once, in time order.
        let packed = files.iter().find(|f| f.ends_with("day-5.parquet")).unwrap();
        let df = crate::store::read_partition(Path::new(packed)).unwrap();
        assert_eq!(df.height(), 3);
        let received = df.column("received_ms").unwrap().i64().unwrap();
        assert!(received.get(0) < received.get(1) && received.get(1) < received.get(2));

        // A second pass is a no-op.
        assert_eq!(consolidate(&parquet, 6).unwrap(), 0);

        drop(store);
        let _ = std::fs::remove_file(&redb);
        let _ = std::fs::remove_dir_all(&parquet);
    }

    #[test]
    fn consolidation_collapses_crash_duplicates_by_seq() {
        let redb = temp("redb");
        let parquet = temp("parquet");
        let store = Store::open(&redb).unwrap();
        store.append_events(&[event(1_000), event(2_000)]).unwrap();

        // Simulate a crash between a merge's write and its delete: the same
        // (seq-stamped) events sit in two partition files on the same day.
        let archived = store.all_events().unwrap();
        let day = parquet.join("1970").join("01").join("01");
        write_partition(&archived, &day.join("events-1.parquet")).unwrap();
        write_partition(&archived, &day.join("day-2.parquet")).unwrap();

        assert_eq!(consolidate(&parquet, 3).unwrap(), 2);
        let files = walk(&parquet);
        assert_eq!(files.len(), 1);
        let df = crate::store::read_partition(Path::new(&files[0])).unwrap();
        assert_eq!(df.height(), 2, "duplicates collapsed by seq");

        drop(store);
        let _ = std::fs::remove_file(&redb);
        let _ = std::fs::remove_dir_all(&parquet);
    }

    #[test]
    fn seals_completed_months_and_leaves_open_ones() {
        const DAY: i64 = 86_400_000;
        let redb = temp("redb");
        let parquet = temp("parquet");
        let store = Store::open(&redb).unwrap();

        // Events in January and February 1970, compacted into day partitions.
        store
            .append_events(&[
                event(1_000),          // Jan 1
                event(5 * DAY),        // Jan 6
                event(31 * DAY + 500), // Feb 1
            ])
            .unwrap();
        assert_eq!(
            compact_window(&store, &parquet, 99_999_999_999, 1).unwrap(),
            3
        );

        // With the WAL cutoff in February, January is final but February is not.
        let cutoff = 32 * DAY;
        assert_eq!(seal_months(&parquet, cutoff, 2).unwrap(), 2);
        let files = walk(&parquet);
        assert_eq!(files.len(), 2);
        let sealed = files
            .iter()
            .find(|f| f.ends_with("01/month-2.parquet"))
            .expect("January sealed at the month level");
        assert!(
            files.iter().any(|f| f.contains("02/01/")),
            "February keeps its day layout"
        );

        // The sealed file holds all of January, in order, and the January day
        // directories are gone.
        let df = crate::store::read_partition(Path::new(sealed)).unwrap();
        assert_eq!(df.height(), 2);
        assert!(!parquet.join("1970").join("01").join("01").exists());

        // Sealing is idempotent...
        assert_eq!(seal_months(&parquet, cutoff, 3).unwrap(), 0);

        // ...and a straggler day written into a sealed month (extreme clock
        // skew) is folded in by the next pass.
        write_partition(
            &store.all_events().unwrap()[..0],
            &parquet
                .join("1970")
                .join("01")
                .join("09")
                .join("events-9.parquet"),
        )
        .unwrap();
        assert_eq!(seal_months(&parquet, cutoff, 4).unwrap(), 2);
        assert!(
            walk(&parquet)
                .iter()
                .any(|f| f.ends_with("01/month-4.parquet"))
        );

        drop(store);
        let _ = std::fs::remove_file(&redb);
        let _ = std::fs::remove_dir_all(&parquet);
    }

    #[test]
    fn retention_removes_months_only_once_fully_expired() {
        const DAY: i64 = 86_400_000;
        let redb = temp("redb");
        let parquet = temp("parquet");
        let store = Store::open(&redb).unwrap();
        store
            .append_events(&[event(1_000), event(35 * DAY)])
            .unwrap();
        assert_eq!(
            compact_window(&store, &parquet, 99_999_999_999, 1).unwrap(),
            2
        );
        assert_eq!(seal_months(&parquet, 90 * DAY, 2).unwrap(), 2);
        assert!(parquet.join("1970").join("01").exists());
        assert!(parquet.join("1970").join("02").exists());

        // A retention window reaching back into January keeps both sealed
        // months (January is only partially expired)...
        let storage = StorageConfig {
            parquet_dir: parquet.to_string_lossy().into_owned(),
            retention: std::time::Duration::from_secs(
                (Utc::now().timestamp() - 20 * 86_400) as u64,
            ),
            ..Default::default()
        };
        enforce_retention(&storage);
        assert!(parquet.join("1970").join("01").exists());

        // ...while one starting after January removes it whole and keeps
        // February.
        let storage = StorageConfig {
            retention: std::time::Duration::from_secs(
                (Utc::now().timestamp() - 40 * 86_400) as u64,
            ),
            ..storage
        };
        enforce_retention(&storage);
        assert!(!parquet.join("1970").join("01").exists());
        assert!(parquet.join("1970").join("02").exists());

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
