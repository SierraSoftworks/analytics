//! One-time migration from the pre-DuckDB stores: the redb hot log + entity
//! tables and the date-partitioned Parquet archive.
//!
//! Runs only when the DuckDB `events` table is empty (a fresh database) and a
//! legacy store exists next to it. The Parquet archive is imported by DuckDB
//! itself (`read_parquet` over the whole tree, de-duplicated by the per-event
//! `seq` since a legacy compactor crash could leave the same events in two
//! files); the redb hot log and entities are read through redb and re-inserted.
//! The legacy files are left in place for the operator to remove after
//! verifying the migration — nothing here deletes user data.
//!
//! A completed migration leaves a marker file beside the legacy redb store.
//! Finding that marker next to an empty database means the database itself
//! was lost (typically `storage.database_path` on non-persistent storage), so
//! the re-import is reported as the data loss it is rather than passing for a
//! first-time upgrade.

use std::path::{Path, PathBuf};

use redb::{ReadableDatabase, ReadableTable, TableDefinition};
use tracing_batteries::prelude::*;

use super::{STORAGE_ADVICE, Store, StoredEvent};
use crate::config::StorageConfig;
use crate::errors::{Result, ResultExt};

const LEGACY_EVENTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("events");
const LEGACY_META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
/// Legacy entity tables and their DuckDB counterparts (same names, same JSON).
const LEGACY_JSON_TABLES: &[&str] = &["projects", "sources", "pixels", "exception_triage"];

/// Import the legacy stores into an empty database, if any exist.
pub(super) fn migrate_if_needed(store: &Store, storage: &StorageConfig) -> Result<()> {
    if store.event_count()? > 0 {
        return Ok(());
    }
    let parquet = has_parquet(Path::new(&storage.parquet_dir));
    let redb = Path::new(&storage.redb_path).exists();
    if !parquet && !redb {
        return Ok(());
    }

    if let Some(previous) = previous_migration(storage) {
        error!(
            "the database at {} is empty, but the legacy stores were already migrated ({previous}): \
             events recorded since then are missing; make sure `storage.database_path` is on \
             persistent storage. Re-importing the legacy stores.",
            storage.database_path().display()
        );
    }

    if parquet {
        let imported = import_parquet(store, &storage.parquet_dir)?;
        info!(
            "migrated {imported} events from the legacy parquet archive at {}",
            storage.parquet_dir
        );
    }
    if redb {
        let (events, entities) = import_redb(store, &storage.redb_path)?;
        info!(
            "migrated {events} hot events and {entities} entities from the legacy redb store at {}",
            storage.redb_path
        );
    }
    store.refresh_next_seq()?;
    record_migration(storage);
    Ok(())
}

/// The marker a completed migration leaves beside the legacy redb store.
fn marker_path(storage: &StorageConfig) -> PathBuf {
    PathBuf::from(format!("{}.migrated", storage.redb_path))
}

/// When, and into which database, the legacy stores were last migrated.
fn previous_migration(storage: &StorageConfig) -> Option<String> {
    std::fs::read_to_string(marker_path(storage))
        .ok()
        .map(|marker| marker.trim().to_string())
}

/// Record the first completed migration. Best-effort: a read-only legacy
/// volume only costs the lost-database warning, never the migration.
fn record_migration(storage: &StorageConfig) {
    let marker = marker_path(storage);
    if marker.exists() {
        return;
    }
    let note = format!(
        "at {} into {}\n",
        chrono::Utc::now().to_rfc3339(),
        storage.database_path().display()
    );
    if let Err(err) = std::fs::write(&marker, note) {
        debug!(
            "could not record the legacy migration at {}: {err}",
            marker.display()
        );
    }
}

/// Whether `dir` contains any `.parquet` file (recursively).
fn has_parquet(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            has_parquet(&path)
        } else {
            path.extension().is_some_and(|e| e == "parquet")
        }
    })
}

/// Import the Parquet archive month directory by month directory, so the
/// working set is one month's partitions rather than the whole (possibly
/// thousands-of-files) tree. `union_by_name` handles partitions written before
/// a column existed; explicit per-column selection (with `NULL` for columns a
/// batch predates) maps the legacy schema onto the events table, and
/// `coalesce` guards the `NOT NULL` columns. De-duplication by `seq` runs as a
/// single pass at the end, only when duplicates actually exist.
fn import_parquet(store: &Store, parquet_dir: &str) -> Result<usize> {
    // One batch per directory that directly holds partition files — a day (or
    // sealed month) at a time, whatever the legacy layout.
    let mut batches: Vec<std::path::PathBuf> = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(parquet_dir)];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut direct = false;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "parquet") {
                direct = true;
            }
        }
        if direct {
            batches.push(dir);
        }
    }
    batches.sort();

    let mut imported = 0;
    for batch in batches {
        imported += import_parquet_batch(store, &batch.to_string_lossy())?;
    }

    // De-duplicate by `seq` only if a crash actually left duplicates in the
    // legacy archive — the overwhelmingly common case is none, keeping the
    // import a pure stream.
    store.with_conn(|conn| {
        let duplicates: i64 = conn
            .query_row(
                "SELECT count(*) - count(DISTINCT seq) FROM events",
                [],
                |row| row.get(0),
            )
            .or_system_err(STORAGE_ADVICE)?;
        if duplicates > 0 {
            let removed = conn
                .execute(
                    "DELETE FROM events WHERE rowid IN (
                         SELECT rowid FROM (
                             SELECT rowid,
                                    row_number() OVER (PARTITION BY seq ORDER BY received_ms) AS n
                             FROM events
                         ) WHERE n > 1
                     )",
                    [],
                )
                .or_system_err(STORAGE_ADVICE)?;
            imported -= removed;
        }
        conn.execute_batch("CHECKPOINT;")
            .or_system_err(STORAGE_ADVICE)?;
        Ok(())
    })?;
    Ok(imported)
}

/// Import the partition files directly inside one directory.
fn import_parquet_batch(store: &Store, dir: &str) -> Result<usize> {
    let glob = format!("{}/*.parquet", dir.replace('\'', "''"));
    store.with_conn(|conn| {
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP VIEW legacy_parquet AS
             SELECT * FROM read_parquet('{glob}', union_by_name = true);"
        ))
        .or_system_err(STORAGE_ADVICE)?;

        // Columns actually present across the archive; anything else reads as NULL.
        let mut stmt = conn
            .prepare("SELECT column_name FROM (DESCRIBE legacy_parquet)")
            .or_system_err(STORAGE_ADVICE)?;
        let present: std::collections::HashSet<String> = stmt
            .query_map([], |row| row.get(0))
            .or_system_err(STORAGE_ADVICE)?
            .collect::<duckdb::Result<_>>()
            .or_system_err(STORAGE_ADVICE)?;
        drop(stmt);

        // One SELECT expression per events-table column, guarding the NOT NULL
        // columns and substituting NULL for columns the whole archive predates.
        // `seq` comes from `seq_expr`, which differs between the two insert
        // passes below.
        let columns = |seq_expr: &str| -> String {
            super::schema::EVENT_COLUMNS
                .iter()
                .map(|name| match *name {
                    "created_ms" if present.contains("created_ms") => {
                        "coalesce(created_ms, received_ms) AS created_ms".to_string()
                    }
                    "created_ms" => "received_ms AS created_ms".to_string(),
                    "seq" => format!("{seq_expr} AS seq"),
                    "bid" | "source" if present.contains(*name) => {
                        format!("coalesce({name}, '') AS {name}")
                    }
                    "bid" | "source" => format!("'' AS {name}"),
                    "kind" if present.contains("kind") => {
                        "coalesce(kind, 'page_load') AS kind".to_string()
                    }
                    "kind" => "'page_load' AS kind".to_string(),
                    "is_unique_user" | "is_unique_page" if present.contains(*name) => {
                        format!("coalesce({name}, false) AS {name}")
                    }
                    "is_unique_user" | "is_unique_page" => format!("false AS {name}"),
                    name if present.contains(name) => name.to_string(),
                    name => format!("NULL AS {name}"),
                })
                .collect::<Vec<_>>()
                .join(", ")
        };

        // The bulk import must stream: a production-sized legacy archive can be
        // thousands of tiny hourly partitions, and both window-function
        // de-duplication and insertion-order preservation would materialize the
        // whole set (the DuckDB guidance recommends disabling the latter for
        // large loads). So: plain streaming inserts, with de-duplication as a
        // rare post-pass only if duplicates are actually present (a legacy
        // compactor crash could leave the same events in two files). Rows
        // without a `seq` (archives predating the column) get fresh numbers in
        // a second, window-based pass over just that (tiny or empty) subset.
        // Also constrain the load itself: wide scan parallelism over thousands
        // of files multiplies per-file decode buffers, and the migration is a
        // one-time batch job where wall-clock hardly matters next to fitting a
        // small container.
        conn.execute_batch(
            "SET preserve_insertion_order = false;
             SET threads = 2;
             SET memory_limit = '192MB';",
        )
        .or_system_err(STORAGE_ADVICE)?;
        let mut imported = 0;
        if present.contains("seq") {
            imported += conn
                .execute(
                    &format!(
                        "INSERT INTO events SELECT {} FROM legacy_parquet WHERE seq IS NOT NULL",
                        columns("seq")
                    ),
                    [],
                )
                .or_system_err(STORAGE_ADVICE)?;
            imported += conn
                .execute(
                    &format!(
                        "INSERT INTO events
                         SELECT {} FROM (
                             SELECT *,
                                    (SELECT coalesce(max(seq), 0) + 1 FROM events)
                                        + row_number() OVER () AS fresh_seq
                             FROM legacy_parquet WHERE seq IS NULL
                         )",
                        columns("fresh_seq")
                    ),
                    [],
                )
                .or_system_err(STORAGE_ADVICE)?;
        } else {
            imported += conn
                .execute(
                    &format!(
                        "INSERT INTO events
                         SELECT {} FROM (
                             SELECT *, row_number() OVER () AS fresh_seq
                             FROM legacy_parquet
                         )",
                        columns("fresh_seq")
                    ),
                    [],
                )
                .or_system_err(STORAGE_ADVICE)?;
        }
        conn.execute_batch(
            "SET preserve_insertion_order = true; RESET threads;
             DROP VIEW legacy_parquet;",
        )
        .or_system_err(STORAGE_ADVICE)?;
        Ok(imported)
    })
}

/// Import the redb hot log (preserving each event's stamped `seq`) and the
/// entity tables (opaque JSON, re-inserted verbatim).
fn import_redb(store: &Store, redb_path: &str) -> Result<(usize, usize)> {
    let db = redb::Database::open(redb_path).or_system_err(STORAGE_ADVICE)?;
    let txn = db.begin_read().or_system_err(STORAGE_ADVICE)?;

    let mut events: Vec<StoredEvent> = Vec::new();
    if let Ok(table) = txn.open_table(LEGACY_EVENTS) {
        for item in table.iter().or_system_err(STORAGE_ADVICE)? {
            let (_key, value) = item.or_system_err(STORAGE_ADVICE)?;
            events.push(serde_json::from_slice(value.value()).or_system_err(STORAGE_ADVICE)?);
        }
    }
    store.import_events(&events)?;

    let mut entities = 0;
    for table_name in LEGACY_JSON_TABLES {
        let def: TableDefinition<&str, &[u8]> = TableDefinition::new(table_name);
        let Ok(table) = txn.open_table(def) else {
            continue;
        };
        for item in table.iter().or_system_err(STORAGE_ADVICE)? {
            let (key, value) = item.or_system_err(STORAGE_ADVICE)?;
            let data = String::from_utf8_lossy(value.value()).into_owned();
            store.with_conn(|conn| {
                conn.execute(
                    &format!("INSERT OR REPLACE INTO {table_name} VALUES (?, ?)"),
                    duckdb::params![key.value(), data],
                )
                .or_system_err(STORAGE_ADVICE)?;
                Ok(())
            })?;
            entities += 1;
        }
    }

    // The exception fingerprint version was raw big-endian bytes in redb meta.
    if let Ok(table) = txn.open_table(LEGACY_META)
        && let Ok(Some(value)) = table.get("fingerprint_version")
    {
        let bytes = value.value();
        let mut buf = [0u8; 4];
        let n = bytes.len().min(4);
        buf[4 - n..].copy_from_slice(&bytes[..n]);
        store.set_fingerprint_version(u32::from_be_bytes(buf))?;
    }

    Ok((events.len(), entities))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::EventKind;

    fn event(received_ms: i64) -> StoredEvent {
        StoredEvent {
            created_ms: received_ms,
            received_ms,
            bid: "b1".to_string(),
            kind: EventKind::PageLoad,
            source: "https://example.com".to_string(),
            ..Default::default()
        }
    }

    /// A scratch directory holding a legacy redb store with one hot event.
    fn legacy_deployment(name: &str) -> (PathBuf, StorageConfig) {
        let dir =
            std::env::temp_dir().join(format!("analytics-legacy-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let redb_path = dir.join("analytics.redb");
        let db = redb::Database::create(&redb_path).unwrap();
        let txn = db.begin_write().unwrap();
        {
            let mut table = txn.open_table(LEGACY_EVENTS).unwrap();
            let json = serde_json::to_vec(&event(1000)).unwrap();
            table.insert(&b"1"[..], json.as_slice()).unwrap();
        }
        txn.commit().unwrap();

        let storage = StorageConfig {
            redb_path: redb_path.to_string_lossy().into_owned(),
            parquet_dir: dir.join("parquet-store").to_string_lossy().into_owned(),
            ..Default::default()
        };
        (dir, storage)
    }

    #[test]
    fn migrates_once_into_a_database_beside_the_legacy_store() {
        let (dir, storage) = legacy_deployment("once");
        assert!(previous_migration(&storage).is_none());

        let store = Store::open_with_migration(&storage).unwrap();
        assert_eq!(store.event_count().unwrap(), 1);
        assert!(storage.database_path().starts_with(&dir));
        assert!(previous_migration(&storage).is_some());
        store.append_events(&[event(2000)]).unwrap();
        drop(store);

        // Reopening the intact database performs no second import.
        let reopened = Store::open_with_migration(&storage).unwrap();
        assert_eq!(reopened.event_count().unwrap(), 2);

        drop(reopened);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lost_database_falls_back_to_the_legacy_history() {
        let (dir, storage) = legacy_deployment("lost");
        let store = Store::open_with_migration(&storage).unwrap();
        store.append_events(&[event(2000)]).unwrap();
        drop(store);
        let migrated = previous_migration(&storage).expect("migration is recorded");

        let database = storage.database_path();
        std::fs::remove_file(&database).unwrap();
        let _ = std::fs::remove_file(database.with_extension("duckdb.wal"));

        // Only the legacy event survives, and the original migration stays on
        // record so every later start keeps reporting the loss.
        let store = Store::open_with_migration(&storage).unwrap();
        assert_eq!(store.event_count().unwrap(), 1);
        assert_eq!(previous_migration(&storage), Some(migrated));

        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
