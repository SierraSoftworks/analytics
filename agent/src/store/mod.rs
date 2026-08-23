//! The durable store: a single embedded DuckDB database holding the append-only
//! event log, the metadata entities, and serving every analytical query.
//!
//! DuckDB collapses what used to be three systems into one: its WAL makes
//! ingest crash-safe (no separate hot log to drain), its columnar storage with
//! zone maps replaces the hand-partitioned Parquet archive (retention is a
//! `DELETE`, not directory surgery), and its vectorized engine with a bounded
//! buffer manager replaces the fold-based query layer and partition cache.
//!
//! The implementation is split across focused modules:
//! - [`schema`] — DDL + instance configuration
//! - [`json`] — generic key→JSON CRUD helpers for the entity tables
//! - [`events`] — append-only event log (appender-based ingest, row mapping)
//! - [`entities`] — project/source/pixel/triage CRUD
//! - [`regroup`] — exception re-fingerprinting
//! - [`legacy`] — one-time migration from the pre-DuckDB redb + Parquet stores
//!
//! Concurrency: a [`Store`] hands out pooled connections ([`Store::with_conn`])
//! cloned from one root connection — they share the database instance (and its
//! buffer manager), so concurrent queries serve from the same cached blocks
//! while DuckDB's MVCC keeps writers isolated.

mod entities;
mod event;
mod events;
mod json;
mod legacy;
mod regroup;
mod schema;
mod triage;

pub use event::{EventKind, StoredEvent};
pub use triage::ExceptionTriage;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use duckdb::Connection;

use crate::config::StorageConfig;
use crate::errors::{Result, ResultExt};

pub(crate) const STORAGE_ADVICE: &[&str] = &[
    "This is an internal storage error.",
    "Retry the operation, and if it persists report it with the server logs.",
];
pub(crate) const OPEN_ADVICE: &[&str] = &[
    "Ensure the data directory exists and is writable.",
    "Make sure no other analytics process has the database open.",
];

/// Idle connections retained for reuse; enough for the web workers' concurrent
/// queries, and a burst beyond it just clones (and later drops) extras.
const POOL_LIMIT: usize = 8;

/// The shared store. Held behind an `Arc`/`web::Data` and never cloned, so the
/// sequence counter stays globally monotonic.
pub struct Store {
    pool: Mutex<Pool>,
    next_seq: AtomicU64,
}

struct Pool {
    root: Connection,
    idle: Vec<Connection>,
}

impl Store {
    /// Open (or create) the store at `path`, ensuring the schema exists.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let root = Connection::open(path.as_ref()).or_system_err(OPEN_ADVICE)?;
        schema::init(&root)?;
        let store = Self {
            pool: Mutex::new(Pool {
                root,
                idle: Vec::new(),
            }),
            next_seq: AtomicU64::new(0),
        };
        store.refresh_next_seq()?;
        Ok(store)
    }

    /// Open the store for serving: apply instance configuration and, on first
    /// run after an upgrade, import the legacy redb hot log and Parquet archive
    /// into the database.
    pub fn open_with_migration(storage: &StorageConfig) -> Result<Self> {
        let store = Self::open(&storage.database_path)?;
        // Migrate first: it applies its own tighter resource settings, and the
        // instance configuration afterwards leaves the configured limits in
        // force for serving.
        legacy::migrate_if_needed(&store, storage)?;
        store.with_conn(|conn| schema::configure(conn, storage))?;
        store.refresh_next_seq()?;
        Ok(store)
    }

    /// Run `f` with a pooled connection. Connections are cloned from the root
    /// (sharing one database instance) and returned to the pool afterwards; a
    /// connection whose closure failed is dropped instead, since it may hold a
    /// broken transaction.
    pub(crate) fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut conn = {
            let mut pool = self.lock_pool();
            match pool.idle.pop() {
                Some(conn) => conn,
                None => pool.root.try_clone().or_system_err(OPEN_ADVICE)?,
            }
        };
        let out = f(&mut conn);
        if out.is_ok() {
            let mut pool = self.lock_pool();
            if pool.idle.len() < POOL_LIMIT {
                pool.idle.push(conn);
            }
        }
        out
    }

    fn lock_pool(&self) -> std::sync::MutexGuard<'_, Pool> {
        self.pool.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Re-derive the next sequence number from the stored maximum. Retention
    /// only ever deletes the *oldest* (lowest-seq) events, so `max(seq)` never
    /// regresses and `max + 1` stays globally monotonic across restarts.
    pub(crate) fn refresh_next_seq(&self) -> Result<()> {
        let next: u64 = self.with_conn(|conn| {
            conn.query_row("SELECT coalesce(max(seq) + 1, 0) FROM events", [], |row| {
                row.get(0)
            })
            .or_system_err(STORAGE_ADVICE)
        })?;
        self.next_seq.store(next, Ordering::SeqCst);
        Ok(())
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
        let path = std::env::temp_dir().join(format!(
            "analytics-test-{}-{}.duckdb",
            std::process::id(),
            n
        ));
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
        let store = temp_store();
        let resolved_at = Utc::now();
        let triage = ExceptionTriage {
            resolved_at: Some(resolved_at),
            muted_at: None,
            note: Some("fixed in v2".to_string()),
            updated_at: resolved_at,
            updated_by: Some("admin".to_string()),
        };
        store.put_triage("p1", "g1", &triage).unwrap();
        let got = store.get_triage("p1", "g1").unwrap().unwrap();
        assert_eq!(got.resolved_at, Some(resolved_at));
        assert_eq!(got.note.as_deref(), Some("fixed in v2"));
        // A different group, or different project, has no triage.
        assert!(store.get_triage("p1", "other").unwrap().is_none());
        assert!(store.get_triage("p2", "g1").unwrap().is_none());
    }

    #[test]
    fn update_triage_upserts_and_keeps_axes_independent() {
        let store = temp_store();
        // The first update creates the record and resolves it.
        store
            .update_triage("p1", "g1", |t| {
                t.resolved_at = Some(Utc::now());
                t.note = Some("first".to_string());
            })
            .unwrap();
        // Muting must not disturb the resolution axis or the note.
        let muted = store
            .update_triage("p1", "g1", |t| t.muted_at = Some(Utc::now()))
            .unwrap();
        assert!(muted.resolved_at.is_some(), "resolution preserved");
        assert!(muted.muted_at.is_some(), "now muted");
        assert_eq!(muted.note.as_deref(), Some("first"), "note preserved");
    }

    #[test]
    fn resolution_regresses_on_a_later_occurrence() {
        let resolved_at = Utc::now();
        let triage = ExceptionTriage {
            resolved_at: Some(resolved_at),
            muted_at: None,
            note: None,
            updated_at: resolved_at,
            updated_by: None,
        };
        let anchor = resolved_at.timestamp_millis();
        // Occurrences up to the anchor keep it resolved…
        assert!(triage.is_resolved(anchor));
        assert!(triage.is_resolved(anchor - 1));
        // …a later one is a regression, surfacing as unresolved again.
        assert!(!triage.is_resolved(anchor + 1));
        assert!(!triage.is_muted());
        // Muting is independent of resolution and of recurrence.
        let muted = ExceptionTriage {
            muted_at: Some(resolved_at),
            resolved_at: None,
            ..triage
        };
        assert!(muted.is_muted());
        assert!(!muted.is_resolved(anchor + 1));
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
        // Round-trip preserves the full event, with seqs stamped in order.
        assert_eq!(all[0].source, "https://a.com");
        assert_eq!(all[0].pathname.as_deref(), Some("/"));
        assert!(all[0].is_unique_user);
        assert!(all.windows(2).all(|w| w[0].seq < w[1].seq));
    }

    #[test]
    fn sequence_is_monotonic_across_reopen() {
        let path = std::env::temp_dir().join(format!(
            "analytics-test-{}-reopen.duckdb",
            std::process::id()
        ));
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
}
