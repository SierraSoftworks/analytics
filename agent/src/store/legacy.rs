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

use std::path::Path;

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

    if has_parquet(Path::new(&storage.parquet_dir)) {
        let imported = import_parquet(store, &storage.parquet_dir)?;
        info!(
            "migrated {imported} events from the legacy parquet archive at {}",
            storage.parquet_dir
        );
    }
    if Path::new(&storage.redb_path).exists() {
        let (events, entities) = import_redb(store, &storage.redb_path)?;
        info!(
            "migrated {events} hot events and {entities} entities from the legacy redb store at {}",
            storage.redb_path
        );
    }
    store.refresh_next_seq()?;
    Ok(())
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
