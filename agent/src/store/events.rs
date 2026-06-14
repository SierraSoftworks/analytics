//! Append-only event log: ingest, scan, and compaction drain.

use std::sync::atomic::Ordering;

use polars::prelude::DataFrame;
use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata};

use super::Store;
use super::event::StoredEvent;
use super::parquet::build_dataframe;
use super::tables::{EVENTS, META, META_NEXT_SEQ, STORAGE_ADVICE, event_key, u64_from_be};
use crate::errors::{Result, ResultExt};

impl Store {
    /// Append a batch of events in a single transaction. Non-blocking ingest is
    /// achieved by the caller feeding this from a background writer task. Each event
    /// is stamped with its monotonic `seq` before being persisted.
    pub fn append_events(&self, events: &[StoredEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }

        let txn = self.db.begin_write().or_system_err(STORAGE_ADVICE)?;
        {
            let mut table = txn.open_table(EVENTS).or_system_err(STORAGE_ADVICE)?;
            for event in events {
                let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
                let key = event_key(event.received_ms, seq);
                let mut stored = event.clone();
                stored.seq = seq;
                let value = serde_json::to_vec(&stored).or_system_err(STORAGE_ADVICE)?;
                table
                    .insert(key.as_slice(), value.as_slice())
                    .or_system_err(STORAGE_ADVICE)?;
            }
        }
        {
            let mut meta = txn.open_table(META).or_system_err(STORAGE_ADVICE)?;
            let seq = self.next_seq.load(Ordering::SeqCst).to_be_bytes();
            meta.insert(META_NEXT_SEQ, seq.as_slice())
                .or_system_err(STORAGE_ADVICE)?;
        }
        txn.commit().or_system_err(STORAGE_ADVICE)?;
        Ok(())
    }

    /// Return every event currently in the hot store (oldest first).
    pub fn all_events(&self) -> Result<Vec<StoredEvent>> {
        let txn = self.db.begin_read().or_system_err(STORAGE_ADVICE)?;
        let table = txn.open_table(EVENTS).or_system_err(STORAGE_ADVICE)?;
        let mut out = Vec::new();
        for item in table.iter().or_system_err(STORAGE_ADVICE)? {
            let (_key, value) = item.or_system_err(STORAGE_ADVICE)?;
            out.push(serde_json::from_slice(value.value()).or_system_err(STORAGE_ADVICE)?);
        }
        Ok(out)
    }

    /// Number of events in the hot store.
    pub fn event_count(&self) -> Result<u64> {
        let txn = self.db.begin_read().or_system_err(STORAGE_ADVICE)?;
        let table = txn.open_table(EVENTS).or_system_err(STORAGE_ADVICE)?;
        table.len().or_system_err(STORAGE_ADVICE)
    }

    /// Read (without removing) all events with `received_ms < threshold_ms`, paired
    /// with their storage keys. The compactor archives these to Parquet and then
    /// deletes *exactly these keys* via [`delete_keys`](Store::delete_keys), so an
    /// event committed after this read (but still below the cutoff) is never deleted
    /// without first being archived.
    pub fn events_before_with_keys(
        &self,
        threshold_ms: i64,
    ) -> Result<Vec<(Vec<u8>, StoredEvent)>> {
        let threshold = threshold_ms.max(0) as u64;
        let txn = self.db.begin_read().or_system_err(STORAGE_ADVICE)?;
        let table = txn.open_table(EVENTS).or_system_err(STORAGE_ADVICE)?;
        let mut out = Vec::new();
        for item in table.iter().or_system_err(STORAGE_ADVICE)? {
            let (key, value) = item.or_system_err(STORAGE_ADVICE)?;
            let key_bytes = key.value();
            if u64_from_be(&key_bytes[0..8]) < threshold {
                out.push((
                    key_bytes.to_vec(),
                    serde_json::from_slice(value.value()).or_system_err(STORAGE_ADVICE)?,
                ));
            }
        }
        Ok(out)
    }

    /// Remove the exact set of event keys (returned by `events_before_with_keys`).
    pub fn delete_keys(&self, keys: &[Vec<u8>]) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        let txn = self.db.begin_write().or_system_err(STORAGE_ADVICE)?;
        {
            let mut table = txn.open_table(EVENTS).or_system_err(STORAGE_ADVICE)?;
            for key in keys {
                table.remove(key.as_slice()).or_system_err(STORAGE_ADVICE)?;
            }
        }
        txn.commit().or_system_err(STORAGE_ADVICE)?;
        Ok(())
    }

    /// Build a polars [`DataFrame`] from the current hot store.
    pub fn hot_dataframe(&self) -> Result<DataFrame> {
        build_dataframe(&self.all_events()?).or_system_err(STORAGE_ADVICE)
    }
}
