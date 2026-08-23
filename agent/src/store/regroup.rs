//! Re-fingerprint stored exceptions when the grouping rules change.
//!
//! The grouping-rules version that was last applied to the data is stamped in
//! the `meta` table. When the running binary reports a different version, every
//! stored exception's `exc_group` is recomputed so historical occurrences merge
//! into the same groups the current rules would produce. The pass is idempotent
//! (recomputing a group yields the same value, and each update is a single SQL
//! statement), so a crash part-way through simply repeats the work on the next
//! start.
//!
//! Note: a client-supplied fingerprint override (`ExceptionReport::fingerprint`)
//! is not persisted, so re-grouping recomputes purely from the stored
//! `(type, message, stack)`. Overrides therefore apply only at ingest time.

use duckdb::params;

use super::entities::META;
use super::{STORAGE_ADVICE, Store};
use crate::errors::{Result, ResultExt};

/// Recomputes a group id from an exception's stored `(type, message, stack)`.
type Regroup = dyn Fn(&str, Option<&str>, Option<&str>) -> String;

impl Store {
    /// The grouping-rules version last applied to the stored data (`0` if never).
    pub fn fingerprint_version(&self) -> Result<u32> {
        Ok(self.get_json(META, "fingerprint_version")?.unwrap_or(0))
    }

    /// Record the grouping-rules version now applied to the stored data.
    pub fn set_fingerprint_version(&self, version: u32) -> Result<()> {
        self.put_json(META, "fingerprint_version", &version)
    }

    /// Recompute `exc_group` for every stored exception, rewriting only the
    /// occurrences whose group actually changes. The distinct `(type, message,
    /// stack)` triples are fingerprinted in Rust and applied as one `UPDATE`
    /// each. Returns the number of changed occurrences.
    pub fn regroup_exceptions(&self, remap: &Regroup) -> Result<usize> {
        let triples: Vec<(Option<String>, Option<String>, Option<String>)> =
            self.with_conn(|conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT DISTINCT exc_type, exc_message, exc_stack
                         FROM events WHERE kind = 'exception'",
                    )
                    .or_system_err(STORAGE_ADVICE)?;
                let rows = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .or_system_err(STORAGE_ADVICE)?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.or_system_err(STORAGE_ADVICE)?);
                }
                Ok(out)
            })?;

        let mut changed = 0;
        for (exc_type, exc_message, exc_stack) in triples {
            let group = remap(
                exc_type.as_deref().unwrap_or(""),
                exc_message.as_deref(),
                exc_stack.as_deref(),
            );
            changed += self.with_conn(|conn| {
                conn.execute(
                    "UPDATE events SET exc_group = ?
                     WHERE kind = 'exception'
                       AND exc_type    IS NOT DISTINCT FROM ?
                       AND exc_message IS NOT DISTINCT FROM ?
                       AND exc_stack   IS NOT DISTINCT FROM ?
                       AND exc_group   IS DISTINCT FROM ?",
                    params![group, exc_type, exc_message, exc_stack, group],
                )
                .or_system_err(STORAGE_ADVICE)
            })?;
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EventKind, StoredEvent};
    use std::path::PathBuf;
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
        let path = temp_path("version.duckdb");
        let store = Store::open(&path).unwrap();
        assert_eq!(store.fingerprint_version().unwrap(), 0);
        store.set_fingerprint_version(3).unwrap();
        assert_eq!(store.fingerprint_version().unwrap(), 3);
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn regroup_updates_only_exceptions_that_change() {
        let path = temp_path("regroup.duckdb");
        let store = Store::open(&path).unwrap();

        // Sits between the two exceptions so the received-ordered `all_events`
        // yields exception, pageview, exception.
        let mut pageview = exception(2_500, "stale");
        pageview.kind = EventKind::PageLoad;
        pageview.exc_type = None;
        pageview.exc_message = None;
        pageview.exc_stack = None;
        pageview.exc_group = None;
        store
            .append_events(&[
                exception(2_000, "stale"),
                pageview,
                exception(3_000, "fresh"),
            ])
            .unwrap();

        // Remap everything to a constant group; only the two exceptions are
        // touched, and only the one whose group actually differs is counted.
        let changed = store
            .regroup_exceptions(&|_, _, _| "fresh".to_string())
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

        // A second pass is a no-op now that every group already matches.
        assert_eq!(
            store
                .regroup_exceptions(&|_, _, _| "fresh".to_string())
                .unwrap(),
            0
        );

        drop(store);
        let _ = std::fs::remove_file(&path);
    }
}
