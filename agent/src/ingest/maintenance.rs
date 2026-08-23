//! Periodic store maintenance: retention enforcement and checkpointing.
//!
//! With DuckDB owning storage, retention is a single `DELETE` — no partition
//! files to consolidate, seal, or sweep. A checkpoint after a purge folds the
//! WAL into the main file and lets the vacated row groups be reused by future
//! appends (DuckDB reuses freed blocks in place; the file does not shrink on
//! disk, it stops growing).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::MissedTickBehavior;
use tracing_batteries::prelude::*;

use crate::config::StorageConfig;
use crate::errors::{Result, ResultExt};
use crate::store::Store;

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
        match tokio::task::spawn_blocking(move || maintain_once(&store, &storage)).await {
            Ok(Ok(0)) => {}
            Ok(Ok(n)) => info!("retention purged {n} expired events"),
            Ok(Err(err)) => error!("store maintenance failed: {err}"),
            Err(err) => error!("store maintenance task panicked: {err}"),
        }
    }
}

/// Delete events past the retention window and checkpoint if anything changed.
/// Returns the number of events removed.
fn maintain_once(store: &Store, storage: &StorageConfig) -> Result<usize> {
    let cutoff = chrono::Utc::now().timestamp_millis()
        - i64::try_from(storage.retention.as_millis()).unwrap_or(i64::MAX);
    let deleted = store.with_conn(|conn| {
        conn.execute(
            &format!("DELETE FROM events WHERE received_ms < {cutoff}"),
            [],
        )
        .or_system_err(crate::store::STORAGE_ADVICE)
    })?;
    if deleted > 0 {
        // Best-effort: a checkpoint can be refused while another transaction
        // is active; the next tick (or DuckDB's automatic WAL threshold) will
        // get it.
        let _ = store.with_conn(|conn| {
            conn.execute_batch("CHECKPOINT")
                .or_system_err(crate::store::STORAGE_ADVICE)
        });
    }
    // One-time note: the legacy stores can be removed once a migrated
    // deployment is verified.
    let legacy = Path::new(&storage.redb_path);
    if legacy.exists() {
        debug!(
            "legacy stores at {} / {} are no longer used and can be deleted once the migration is verified",
            storage.redb_path, storage.parquet_dir
        );
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EventKind, StoredEvent};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp() -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "analytics-maintenance-{}-{}.duckdb",
            std::process::id(),
            n
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
    fn retention_purges_only_expired_events() {
        let path = temp();
        let store = Store::open(&path).unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        store
            .append_events(&[
                event(now - 10 * 86_400_000), // expired under a 5-day window
                event(now - 86_400_000),      // retained
                event(now),                   // retained
            ])
            .unwrap();

        let storage = StorageConfig {
            retention: Duration::from_secs(5 * 86_400),
            ..Default::default()
        };
        assert_eq!(maintain_once(&store, &storage).unwrap(), 1);
        assert_eq!(store.event_count().unwrap(), 2);
        // A second pass finds nothing to do.
        assert_eq!(maintain_once(&store, &storage).unwrap(), 0);

        drop(store);
        let _ = std::fs::remove_file(&path);
    }
}
