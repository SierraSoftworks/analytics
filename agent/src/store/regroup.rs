//! Re-fingerprint stored exceptions when the grouping rules change.
//!
//! The grouping-rules version that was last applied to the data is stamped in the
//! `meta` table. When the running binary reports a different version, every stored
//! exception's `exc_group` is recomputed — across both the redb hot store and the
//! archived Parquet partitions — so historical occurrences merge into the same
//! groups the current rules would produce. The pass is idempotent (recomputing a
//! group yields the same value, and partitions are rewritten atomically), so a
//! crash part-way through simply repeats the work on the next start.
//!
//! Note: a client-supplied fingerprint override (`ExceptionReport::fingerprint`) is
//! not persisted, so re-grouping recomputes purely from the stored
//! `(type, message, stack)`. Overrides therefore apply only at ingest time.

use std::path::{Path, PathBuf};

use redb::{ReadableDatabase, ReadableTable};

use super::Store;
use super::event::{EventKind, StoredEvent};
use super::tables::{EVENTS, META, META_FINGERPRINT_VERSION, STORAGE_ADVICE, u32_from_be};
use crate::errors::{Result, ResultExt};

/// Recomputes a group id from an exception's stored `(type, message, stack)`.
type Regroup = dyn Fn(&str, Option<&str>, Option<&str>) -> String;

impl Store {
    /// The grouping-rules version last applied to the stored data (`0` if never).
    pub fn fingerprint_version(&self) -> Result<u32> {
        let txn = self.db.begin_read().or_system_err(STORAGE_ADVICE)?;
        let table = txn.open_table(META).or_system_err(STORAGE_ADVICE)?;
        match table
            .get(META_FINGERPRINT_VERSION)
            .or_system_err(STORAGE_ADVICE)?
        {
            Some(value) => Ok(u32_from_be(value.value())),
            None => Ok(0),
        }
    }

    /// Record the grouping-rules version now applied to the stored data.
    pub fn set_fingerprint_version(&self, version: u32) -> Result<()> {
        let txn = self.db.begin_write().or_system_err(STORAGE_ADVICE)?;
        {
            let mut table = txn.open_table(META).or_system_err(STORAGE_ADVICE)?;
            table
                .insert(META_FINGERPRINT_VERSION, version.to_be_bytes().as_slice())
                .or_system_err(STORAGE_ADVICE)?;
        }
        txn.commit().or_system_err(STORAGE_ADVICE)?;
        Ok(())
    }

    /// Recompute `exc_group` for every exception in the redb hot store, rewriting
    /// only the events whose group changes. Returns the number of changed
    /// occurrences.
    pub fn regroup_hot_exceptions(&self, remap: &Regroup) -> Result<usize> {
        let txn = self.db.begin_write().or_system_err(STORAGE_ADVICE)?;
        let changed;
        {
            let mut table = txn.open_table(EVENTS).or_system_err(STORAGE_ADVICE)?;

            // Collect the rewrites first: the iterator holds an immutable borrow of
            // the table that must end before we can insert the updates back.
            let mut updates: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
            for item in table.iter().or_system_err(STORAGE_ADVICE)? {
                let (key, value) = item.or_system_err(STORAGE_ADVICE)?;
                let mut event: StoredEvent =
                    serde_json::from_slice(value.value()).or_system_err(STORAGE_ADVICE)?;
                if event.kind != EventKind::Exception {
                    continue;
                }
                let group = remap(
                    event.exc_type.as_deref().unwrap_or(""),
                    event.exc_message.as_deref(),
                    event.exc_stack.as_deref(),
                );
                if event.exc_group.as_deref() != Some(group.as_str()) {
                    event.exc_group = Some(group);
                    let bytes = serde_json::to_vec(&event).or_system_err(STORAGE_ADVICE)?;
                    updates.push((key.value().to_vec(), bytes));
                }
            }

            changed = updates.len();
            for (key, bytes) in &updates {
                table
                    .insert(key.as_slice(), bytes.as_slice())
                    .or_system_err(STORAGE_ADVICE)?;
            }
        }
        txn.commit().or_system_err(STORAGE_ADVICE)?;
        Ok(changed)
    }

    /// Recompute `exc_group` for every exception in the archived Parquet partitions,
    /// rewriting only the partitions that actually change. Returns the number of
    /// changed occurrences.
    pub fn regroup_cold_exceptions(&self, parquet_dir: &str, remap: &Regroup) -> Result<usize> {
        let root = Path::new(parquet_dir);
        if !root.exists() {
            return Ok(0);
        }
        let mut total = 0;
        for file in parquet_files(root) {
            total += super::parquet::regroup_partition(&file, remap)?;
        }
        Ok(total)
    }
}

/// Every `*.parquet` partition under `root` (recursively); `.tmp` writes-in-progress
/// are skipped by the extension filter.
fn parquet_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "parquet") {
                out.push(path);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_path(suffix: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "analytics-regroup-{}-{}-{}",
            std::process::id(),
            n,
            suffix
        ))
    }

    fn exception(received_ms: i64, group: &str) -> StoredEvent {
        StoredEvent {
            received_ms,
            created_ms: received_ms,
            kind: EventKind::Exception,
            source: "https://example.com".into(),
            exc_type: Some("TypeError".into()),
            exc_message: Some("boom".into()),
            exc_stack: Some("at handler (app.js:1:2)".into()),
            exc_group: Some(group.into()),
            ..Default::default()
        }
    }

    #[test]
    fn fingerprint_version_roundtrips() {
        let path = temp_path("version.redb");
        let store = Store::open(&path).unwrap();
        assert_eq!(store.fingerprint_version().unwrap(), 0);
        store.set_fingerprint_version(3).unwrap();
        assert_eq!(store.fingerprint_version().unwrap(), 3);
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn regroup_hot_updates_only_exceptions() {
        let path = temp_path("hot.redb");
        let store = Store::open(&path).unwrap();

        // Sits between the two exceptions so the received_ms-ordered `all_events`
        // yields exception, pageview, exception.
        let mut pageview = exception(2_500, "stale");
        pageview.kind = EventKind::PageLoad;
        pageview.exc_type = None;
        pageview.exc_message = None;
        pageview.exc_stack = None;
        pageview.exc_group = None;
        store
            .append_events(&[exception(2_000, "stale"), pageview, exception(3_000, "fresh")])
            .unwrap();

        // Remap everything to a constant group; only the two exceptions are touched,
        // and only the one whose group actually differs is counted as changed.
        let changed = store
            .regroup_hot_exceptions(&|_, _, _| "fresh".to_string())
            .unwrap();
        assert_eq!(changed, 1);

        let groups: Vec<Option<String>> = store
            .all_events()
            .unwrap()
            .into_iter()
            .map(|e| e.exc_group)
            .collect();
        assert_eq!(
            groups,
            vec![Some("fresh".to_string()), None, Some("fresh".to_string())]
        );

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn regroup_cold_rewrites_partitions() {
        let redb = temp_path("cold.redb");
        let parquet = temp_path("cold-parquet");
        let store = Store::open(&redb).unwrap();

        let file = parquet.join("2025").join("01").join("01").join("events-1.parquet");
        super::super::write_partition(&[exception(1_000, "stale"), exception(2_000, "stale")], &file)
            .unwrap();

        let changed = store
            .regroup_cold_exceptions(parquet.to_str().unwrap(), &|_, _, _| "fresh".to_string())
            .unwrap();
        assert_eq!(changed, 2);

        let df = super::super::read_partition(&file).unwrap();
        let groups = df.column("exc_group").unwrap().str().unwrap();
        assert!((0..df.height()).all(|i| groups.get(i) == Some("fresh")));

        // A second pass is a no-op now that every group already matches.
        assert_eq!(
            store
                .regroup_cold_exceptions(parquet.to_str().unwrap(), &|_, _, _| "fresh".to_string())
                .unwrap(),
            0
        );

        drop(store);
        let _ = std::fs::remove_file(&redb);
        let _ = std::fs::remove_dir_all(&parquet);
    }
}
