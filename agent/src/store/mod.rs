//! The durable store: a redb database holding metadata tables plus an append-only,
//! hot event log. Old events are drained to Parquet by the compactor.
//!
//! The implementation is split across focused modules:
//! - [`tables`] — table definitions, key encoding, shared advice
//! - [`schema`] — on-disk version + forward migrations
//! - [`json`] — generic JSON CRUD helpers
//! - [`events`] — append-only event log
//! - [`entities`] — project/source/pixel/triage CRUD
//! - [`parquet`] — columnar Parquet bridge

mod entities;
mod event;
mod events;
mod json;
mod parquet;
mod schema;
mod tables;
mod triage;

pub use event::{EventKind, StoredEvent};
pub use parquet::{build_dataframe, read_partition, write_partition};
pub use triage::ExceptionTriage;

use std::path::Path;
use std::sync::atomic::AtomicU64;

use redb::{Database, ReadableDatabase};

use crate::errors::{Result, ResultExt};

/// The shared store. Held behind an `Arc`/`web::Data` and never cloned, so the
/// sequence counter stays globally monotonic.
pub struct Store {
    db: Database,
    next_seq: AtomicU64,
}

impl Store {
    /// Open (or create) the store at `path`, ensuring tables exist and the schema is
    /// migrated to the current version.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Database::create(path).or_system_err(tables::OPEN_ADVICE)?;
        ensure_tables(&db)?;
        schema::migrate(&db)?;
        let next_seq = read_next_seq(&db)?;
        Ok(Self {
            db,
            next_seq: AtomicU64::new(next_seq),
        })
    }
}

/// Touch every table so it exists for later read transactions.
fn ensure_tables(db: &Database) -> Result<()> {
    let txn = db.begin_write().or_system_err(tables::OPEN_ADVICE)?;
    txn.open_table(tables::EVENTS).or_system_err(tables::OPEN_ADVICE)?;
    txn.open_table(tables::PROJECTS).or_system_err(tables::OPEN_ADVICE)?;
    txn.open_table(tables::SOURCES).or_system_err(tables::OPEN_ADVICE)?;
    txn.open_table(tables::PIXELS).or_system_err(tables::OPEN_ADVICE)?;
    txn.open_table(tables::EXCEPTION_TRIAGE)
        .or_system_err(tables::OPEN_ADVICE)?;
    txn.open_table(tables::META).or_system_err(tables::OPEN_ADVICE)?;
    txn.commit().or_system_err(tables::OPEN_ADVICE)?;
    Ok(())
}

fn read_next_seq(db: &Database) -> Result<u64> {
    let txn = db.begin_read().or_system_err(tables::OPEN_ADVICE)?;
    let table = txn.open_table(tables::META).or_system_err(tables::OPEN_ADVICE)?;
    match table
        .get(tables::META_NEXT_SEQ)
        .or_system_err(tables::STORAGE_ADVICE)?
    {
        Some(value) => Ok(tables::u64_from_be(value.value())),
        None => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use analytics_api::Project;
    use chrono::Utc;
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

    fn event(source: &str, received_ms: i64) -> StoredEvent {
        StoredEvent {
            created_ms: received_ms,
            received_ms,
            bid: "b1".to_string(),
            kind: EventKind::PageLoad,
            source: source.to_string(),
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
            .append_events(&[event("https://a.com", 1000), event("https://b.com", 2000)])
            .unwrap();
        store.append_events(&[event("https://a.com", 3000)]).unwrap();
        assert_eq!(store.event_count().unwrap(), 3);
        let all = store.all_events().unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].received_ms, 1000);
        assert_eq!(all[2].received_ms, 3000);
    }

    #[test]
    fn sequence_is_monotonic_across_reopen() {
        let path = std::env::temp_dir()
            .join(format!("analytics-test-{}-reopen.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let store = Store::open(&path).unwrap();
            store.append_events(&[event("https://a.com", 1000)]).unwrap();
        }
        let reopened = Store::open(&path).unwrap();
        assert!(reopened.next_seq.load(Ordering::SeqCst) >= 1);
        drop(reopened);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn take_before_drains_old_events() {
        let store = temp_store();
        store
            .append_events(&[
                event("https://a", 1000),
                event("https://b", 2000),
                event("https://c", 3000),
            ])
            .unwrap();
        let drained = store.take_before(2500).unwrap();
        assert_eq!(drained.len(), 2);
        assert_eq!(store.event_count().unwrap(), 1);
        assert_eq!(store.all_events().unwrap()[0].received_ms, 3000);
    }

    #[test]
    fn parquet_roundtrip() {
        let store = temp_store();
        let events = vec![event("https://a.com", 1000), event("pixel://01HX", 2000)];
        store.append_events(&events).unwrap();
        let path = std::env::temp_dir()
            .join(format!("analytics-test-{}-part.parquet", std::process::id()));
        super::write_partition(&events, &path).unwrap();
        let df = super::read_partition(&path).unwrap();
        assert_eq!(df.height(), 2);
        assert!(df.get_column_names().iter().any(|c| c.as_str() == "source"));
        let _ = std::fs::remove_file(&path);
    }
}
