mod error;
mod event;
mod parquet;

pub use error::StoreError;
pub use event::{EventKind, StoredEvent};
pub use parquet::{build_dataframe, read_partition, write_partition};

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use analytics_api::{ExceptionStatus, Pixel, Project, Source};
use chrono::{DateTime, Utc};
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Serialize, de::DeserializeOwned};

/// JSON-valued, string-keyed table (projects, sources, pixels, triage, meta).
type JsonTable = TableDefinition<'static, &'static str, &'static [u8]>;

const EVENTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("events");
const PROJECTS: JsonTable = TableDefinition::new("projects");
const SOURCES: JsonTable = TableDefinition::new("sources");
const PIXELS: JsonTable = TableDefinition::new("pixels");
const EXCEPTION_TRIAGE: JsonTable = TableDefinition::new("exception_triage");
const META: JsonTable = TableDefinition::new("meta");

const META_NEXT_SEQ: &str = "next_seq";

/// Admin-set triage state for an exception group, keyed by `(project_id, group_id)`.
/// This is the only mutable exception state; occurrences are append-only events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ExceptionTriage {
    pub status: ExceptionStatus,
    pub note: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<String>,
}

/// The durable store: a redb database holding metadata tables plus an append-only,
/// hot event log. Old events are drained to Parquet by the compactor. The store is
/// shared (behind an `Arc`/`web::Data`) and never cloned, so the sequence counter
/// stays globally monotonic.
pub struct Store {
    db: Database,
    next_seq: AtomicU64,
}

impl Store {
    /// Open (or create) the store at `path`, ensuring all tables exist.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db = Database::create(path)?;

        let next_seq = {
            let txn = db.begin_write()?;
            // Touch every table so it exists for later read transactions.
            txn.open_table(EVENTS)?;
            txn.open_table(PROJECTS)?;
            txn.open_table(SOURCES)?;
            txn.open_table(PIXELS)?;
            txn.open_table(EXCEPTION_TRIAGE)?;
            let seq = {
                let meta = txn.open_table(META)?;
                match meta.get(META_NEXT_SEQ)? {
                    Some(v) => u64_from_be(v.value()),
                    None => 0,
                }
            };
            txn.commit()?;
            seq
        };

        Ok(Self {
            db,
            next_seq: AtomicU64::new(next_seq),
        })
    }

    // ----------------------------------------------------------------- events

    /// Append a batch of events in a single transaction. Non-blocking writes are
    /// achieved by the caller feeding this from a background task.
    pub fn append_events(&self, events: &[StoredEvent]) -> Result<(), StoreError> {
        if events.is_empty() {
            return Ok(());
        }

        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(EVENTS)?;
            for event in events {
                let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
                let key = event_key(event.received_ms, seq);
                let value = serde_json::to_vec(event)?;
                table.insert(key.as_slice(), value.as_slice())?;
            }
        }
        {
            let mut meta = txn.open_table(META)?;
            let seq = self.next_seq.load(Ordering::SeqCst).to_be_bytes();
            meta.insert(META_NEXT_SEQ, seq.as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Return every event currently in the hot store (oldest first).
    pub fn all_events(&self) -> Result<Vec<StoredEvent>, StoreError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(EVENTS)?;
        let mut out = Vec::new();
        for item in table.iter()? {
            let (_key, value) = item?;
            out.push(serde_json::from_slice(value.value())?);
        }
        Ok(out)
    }

    /// Number of events in the hot store.
    pub fn event_count(&self) -> Result<u64, StoreError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(EVENTS)?;
        Ok(table.len()?)
    }

    /// Remove and return all events with `received_ms < threshold_ms` (used by the
    /// compactor to seal a time window into Parquet).
    pub fn take_before(&self, threshold_ms: i64) -> Result<Vec<StoredEvent>, StoreError> {
        let threshold = threshold_ms.max(0) as u64;

        let mut keys = Vec::new();
        let mut events = Vec::new();
        {
            let txn = self.db.begin_read()?;
            let table = txn.open_table(EVENTS)?;
            for item in table.iter()? {
                let (key, value) = item?;
                let key_bytes = key.value();
                let received = u64_from_be(&key_bytes[0..8]);
                if received < threshold {
                    keys.push(key_bytes.to_vec());
                    events.push(serde_json::from_slice(value.value())?);
                }
            }
        }

        if !keys.is_empty() {
            let txn = self.db.begin_write()?;
            {
                let mut table = txn.open_table(EVENTS)?;
                for key in &keys {
                    table.remove(key.as_slice())?;
                }
            }
            txn.commit()?;
        }

        Ok(events)
    }

    /// Build a polars [`DataFrame`] from the current hot store.
    pub fn hot_dataframe(&self) -> Result<polars::prelude::DataFrame, StoreError> {
        Ok(build_dataframe(&self.all_events()?)?)
    }

    // --------------------------------------------------------------- projects

    pub fn put_project(&self, project: &Project) -> Result<(), StoreError> {
        self.put_json(PROJECTS, &project.id, project)
    }
    pub fn get_project(&self, id: &str) -> Result<Option<Project>, StoreError> {
        self.get_json(PROJECTS, id)
    }
    pub fn list_projects(&self) -> Result<Vec<Project>, StoreError> {
        self.list_json(PROJECTS)
    }
    pub fn delete_project(&self, id: &str) -> Result<bool, StoreError> {
        self.delete_key(PROJECTS, id)
    }

    // ---------------------------------------------------------------- sources

    pub fn put_source(&self, source: &Source) -> Result<(), StoreError> {
        self.put_json(SOURCES, &source.hostname, source)
    }
    pub fn get_source(&self, hostname: &str) -> Result<Option<Source>, StoreError> {
        self.get_json(SOURCES, hostname)
    }
    pub fn list_sources(&self) -> Result<Vec<Source>, StoreError> {
        self.list_json(SOURCES)
    }
    pub fn delete_source(&self, hostname: &str) -> Result<bool, StoreError> {
        self.delete_key(SOURCES, hostname)
    }

    // ----------------------------------------------------------------- pixels

    pub fn put_pixel(&self, pixel: &Pixel) -> Result<(), StoreError> {
        self.put_json(PIXELS, &pixel.id, pixel)
    }
    pub fn get_pixel(&self, id: &str) -> Result<Option<Pixel>, StoreError> {
        self.get_json(PIXELS, id)
    }
    pub fn list_pixels(&self) -> Result<Vec<Pixel>, StoreError> {
        self.list_json(PIXELS)
    }
    pub fn delete_pixel(&self, id: &str) -> Result<bool, StoreError> {
        self.delete_key(PIXELS, id)
    }

    // ------------------------------------------------------- exception triage

    pub fn put_triage(
        &self,
        project_id: &str,
        group_id: &str,
        triage: &ExceptionTriage,
    ) -> Result<(), StoreError> {
        self.put_json(EXCEPTION_TRIAGE, &triage_key(project_id, group_id), triage)
    }
    pub fn get_triage(
        &self,
        project_id: &str,
        group_id: &str,
    ) -> Result<Option<ExceptionTriage>, StoreError> {
        self.get_json(EXCEPTION_TRIAGE, &triage_key(project_id, group_id))
    }

    // ------------------------------------------------- generic JSON helpers

    fn put_json<T: Serialize>(
        &self,
        def: JsonTable,
        key: &str,
        value: &T,
    ) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec(value)?;
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(def)?;
            table.insert(key, bytes.as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    fn get_json<T: DeserializeOwned>(
        &self,
        def: JsonTable,
        key: &str,
    ) -> Result<Option<T>, StoreError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(def)?;
        match table.get(key)? {
            Some(value) => Ok(Some(serde_json::from_slice(value.value())?)),
            None => Ok(None),
        }
    }

    fn list_json<T: DeserializeOwned>(&self, def: JsonTable) -> Result<Vec<T>, StoreError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(def)?;
        let mut out = Vec::new();
        for item in table.iter()? {
            let (_key, value) = item?;
            out.push(serde_json::from_slice(value.value())?);
        }
        Ok(out)
    }

    fn delete_key(&self, def: JsonTable, key: &str) -> Result<bool, StoreError> {
        let txn = self.db.begin_write()?;
        let existed = {
            let mut table = txn.open_table(def)?;
            table.remove(key)?.is_some()
        };
        txn.commit()?;
        Ok(existed)
    }
}

fn event_key(received_ms: i64, seq: u64) -> [u8; 16] {
    let mut key = [0u8; 16];
    key[0..8].copy_from_slice(&(received_ms.max(0) as u64).to_be_bytes());
    key[8..16].copy_from_slice(&seq.to_be_bytes());
    key
}

fn u64_from_be(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let n = bytes.len().min(8);
    buf[8 - n..].copy_from_slice(&bytes[..n]);
    u64::from_be_bytes(buf)
}

/// Composite key for the triage table. The unit-separator byte cannot appear in
/// ULIDs or hostnames, so it is a safe delimiter.
fn triage_key(project_id: &str, group_id: &str) -> String {
    format!("{project_id}\u{1f}{group_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempStore {
        store: Store,
        path: std::path::PathBuf,
    }

    impl std::ops::Deref for TempStore {
        type Target = Store;
        fn deref(&self) -> &Store {
            &self.store
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn temp_store() -> TempStore {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("analytics-test-{}-{}.redb", std::process::id(), n));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path).expect("open store");
        TempStore { store, path }
    }

    fn event(hostname: &str, received_ms: i64) -> StoredEvent {
        StoredEvent {
            created_ms: received_ms,
            received_ms,
            bid: "b1".to_string(),
            kind: EventKind::PageLoad,
            hostname: Some(hostname.to_string()),
            pathname: Some("/".to_string()),
            is_unique_user: true,
            ..Default::default()
        }
    }

    #[test]
    fn project_crud_roundtrip() {
        let store = temp_store();
        let project = Project {
            id: "p1".to_string(),
            name: "Example".to_string(),
            slug: "example".to_string(),
            created_at: Utc::now(),
        };
        store.put_project(&project).unwrap();
        assert_eq!(store.get_project("p1").unwrap().as_ref(), Some(&project));
        assert_eq!(store.list_projects().unwrap().len(), 1);
        assert!(store.delete_project("p1").unwrap());
        assert!(store.get_project("p1").unwrap().is_none());
    }

    #[test]
    fn appends_and_reads_events() {
        let store = temp_store();
        store
            .append_events(&[event("a.com", 1000), event("b.com", 2000)])
            .unwrap();
        store.append_events(&[event("a.com", 3000)]).unwrap();
        assert_eq!(store.event_count().unwrap(), 3);
        let all = store.all_events().unwrap();
        assert_eq!(all.len(), 3);
        // Oldest first.
        assert_eq!(all[0].received_ms, 1000);
        assert_eq!(all[2].received_ms, 3000);
    }

    #[test]
    fn sequence_is_monotonic_across_reopen() {
        // redb holds an exclusive lock, so the first handle must be dropped before
        // reopening (mirrors production, where the store is opened once at startup).
        let path = std::env::temp_dir()
            .join(format!("analytics-test-{}-reopen.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let store = Store::open(&path).unwrap();
            store.append_events(&[event("a.com", 1000)]).unwrap();
        }
        let reopened = Store::open(&path).unwrap();
        // next_seq was persisted and reloaded, so it is at least 1.
        assert!(reopened.next_seq.load(Ordering::SeqCst) >= 1);
        drop(reopened);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn take_before_drains_old_events() {
        let store = temp_store();
        store
            .append_events(&[event("a", 1000), event("b", 2000), event("c", 3000)])
            .unwrap();
        let drained = store.take_before(2500).unwrap();
        assert_eq!(drained.len(), 2);
        assert_eq!(store.event_count().unwrap(), 1);
        assert_eq!(store.all_events().unwrap()[0].received_ms, 3000);
    }

    #[test]
    fn parquet_roundtrip() {
        let store = temp_store();
        let events = vec![event("a.com", 1000), event("b.com", 2000)];
        store.append_events(&events).unwrap();
        let path = std::env::temp_dir().join(format!(
            "analytics-test-{}-part.parquet",
            std::process::id()
        ));
        super::write_partition(&events, &path).unwrap();
        let df = super::read_partition(&path).unwrap();
        assert_eq!(df.height(), 2);
        assert!(df.get_column_names().iter().any(|c| c.as_str() == "hostname"));
        let _ = std::fs::remove_file(&path);
    }
}
