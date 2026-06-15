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
    txn.open_table(tables::EVENTS)
        .or_system_err(tables::OPEN_ADVICE)?;
    txn.open_table(tables::PROJECTS)
        .or_system_err(tables::OPEN_ADVICE)?;
    txn.open_table(tables::SOURCES)
        .or_system_err(tables::OPEN_ADVICE)?;
    txn.open_table(tables::PIXELS)
        .or_system_err(tables::OPEN_ADVICE)?;
    txn.open_table(tables::EXCEPTION_TRIAGE)
        .or_system_err(tables::OPEN_ADVICE)?;
    txn.open_table(tables::META)
        .or_system_err(tables::OPEN_ADVICE)?;
    txn.commit().or_system_err(tables::OPEN_ADVICE)?;
    Ok(())
}

fn read_next_seq(db: &Database) -> Result<u64> {
    let txn = db.begin_read().or_system_err(tables::OPEN_ADVICE)?;
    let table = txn
        .open_table(tables::META)
        .or_system_err(tables::OPEN_ADVICE)?;
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
    fn triage_roundtrip() {
        use analytics_api::ExceptionStatus;
        let store = temp_store();
        let triage = ExceptionTriage {
            status: ExceptionStatus::Resolved,
            note: Some("fixed in v2".to_string()),
            updated_at: Utc::now(),
            updated_by: Some("admin".to_string()),
        };
        store.put_triage("p1", "g1", &triage).unwrap();
        let got = store.get_triage("p1", "g1").unwrap().unwrap();
        assert_eq!(got.status, ExceptionStatus::Resolved);
        assert_eq!(got.note.as_deref(), Some("fixed in v2"));
        // A different group, or different project, has no triage.
        assert!(store.get_triage("p1", "other").unwrap().is_none());
        assert!(store.get_triage("p2", "g1").unwrap().is_none());
    }

    #[test]
    fn appends_and_reads_events() {
        let store = temp_store();
        store
            .append_events(&[event("https://a.com", 1000), event("https://b.com", 2000)])
            .unwrap();
        store
            .append_events(&[event("https://a.com", 3000)])
            .unwrap();
        assert_eq!(store.event_count().unwrap(), 3);
        let all = store.all_events().unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].received_ms, 1000);
        assert_eq!(all[2].received_ms, 3000);
    }

    #[test]
    fn sequence_is_monotonic_across_reopen() {
        let path =
            std::env::temp_dir().join(format!("analytics-test-{}-reopen.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let store = Store::open(&path).unwrap();
            store
                .append_events(&[event("https://a.com", 1000)])
                .unwrap();
        }
        let reopened = Store::open(&path).unwrap();
        assert!(reopened.next_seq.load(Ordering::SeqCst) >= 1);
        drop(reopened);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn events_before_with_keys_then_delete_keys() {
        let store = temp_store();
        store
            .append_events(&[
                event("https://a", 1000),
                event("https://b", 2000),
                event("https://c", 3000),
            ])
            .unwrap();
        let pairs = store.events_before_with_keys(2500).unwrap();
        assert_eq!(pairs.len(), 2);
        let keys: Vec<Vec<u8>> = pairs.into_iter().map(|(key, _)| key).collect();
        store.delete_keys(&keys).unwrap();
        assert_eq!(store.event_count().unwrap(), 1);
        assert_eq!(store.all_events().unwrap()[0].received_ms, 3000);
    }

    #[test]
    fn mutate_source_is_atomic_and_reports_absence() {
        use analytics_api::Source;
        let store = temp_store();
        store.register_source_if_absent("https://a.com").unwrap();

        let updated: Option<Source> = store
            .mutate_source("https://a.com", |s| {
                s.project_id = Some("p1".to_string());
                s.display_name = Some("A".to_string());
            })
            .unwrap();
        assert_eq!(updated.unwrap().project_id.as_deref(), Some("p1"));
        assert_eq!(
            store
                .get_source("https://a.com")
                .unwrap()
                .unwrap()
                .project_id
                .as_deref(),
            Some("p1")
        );

        // A missing URI returns None and does not create a row.
        let missing = store
            .mutate_source("https://missing", |s| s.project_id = Some("x".to_string()))
            .unwrap();
        assert!(missing.is_none());
        assert!(store.get_source("https://missing").unwrap().is_none());
    }

    #[test]
    fn delete_project_cascade_unassigns_sources_and_removes_pixels() {
        use analytics_api::{Pixel, Source, SourceKind};
        let store = temp_store();
        let now = Utc::now();
        store
            .put_project(&Project {
                id: "p1".to_string(),
                name: "P".to_string(),
                slug: "p".to_string(),
                created_at: now,
            })
            .unwrap();
        store
            .put_source(&Source {
                uri: "https://a.com".to_string(),
                project_id: Some("p1".to_string()),
                kind: SourceKind::Website,
                display_name: None,
                created_at: now,
                first_seen: Some(now),
                last_seen: Some(now),
            })
            .unwrap();
        store
            .put_pixel(&Pixel {
                id: "px1".to_string(),
                project_id: "p1".to_string(),
                name: "n".to_string(),
                event_name: "pixel".to_string(),
                metadata: Default::default(),
                created_at: now,
                last_hit: None,
            })
            .unwrap();

        assert!(store.delete_project_cascade("p1").unwrap());
        assert!(store.get_project("p1").unwrap().is_none());
        assert_eq!(
            store
                .get_source("https://a.com")
                .unwrap()
                .unwrap()
                .project_id,
            None
        );
        assert!(store.get_pixel("px1").unwrap().is_none());
        // A second source not on the project is left untouched; unknown id -> false.
        assert!(!store.delete_project_cascade("nope").unwrap());
    }

    #[test]
    fn parquet_roundtrip() {
        let store = temp_store();
        let events = vec![event("https://a.com", 1000), event("pixel://01HX", 2000)];
        store.append_events(&events).unwrap();
        let path = std::env::temp_dir().join(format!(
            "analytics-test-{}-part.parquet",
            std::process::id()
        ));
        super::write_partition(&events, &path).unwrap();
        let df = super::read_partition(&path).unwrap();
        assert_eq!(df.height(), 2);
        assert!(df.get_column_names().iter().any(|c| c.as_str() == "source"));
        let _ = std::fs::remove_file(&path);
    }
}
