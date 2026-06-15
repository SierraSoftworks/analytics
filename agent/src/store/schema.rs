//! On-disk schema versioning and forward migrations.
//!
//! The current version is stamped in the `meta` table. On open, [`migrate`] applies
//! any pending migration steps in order. A database newer than this build is
//! rejected rather than silently misread.

use redb::{Database, ReadableDatabase};

use super::tables::{META, META_SCHEMA_VERSION, OPEN_ADVICE, STORAGE_ADVICE, u32_from_be};
use crate::errors::{Result, ResultExt};

/// The current on-disk schema version. Bump this and add an [`apply`] arm whenever
/// the stored layout changes incompatibly.
pub(super) const SCHEMA_VERSION: u32 = 1;

/// Ensure the database is at [`SCHEMA_VERSION`], applying migrations in order.
pub(super) fn migrate(db: &Database) -> Result<()> {
    let current = read_version(db)?;

    if current == SCHEMA_VERSION {
        return Ok(());
    }

    if current > SCHEMA_VERSION {
        return Err(human_errors::user(
            format!(
                "The analytics data store is at schema v{current}, but this build only supports v{SCHEMA_VERSION}."
            ),
            &[
                "Upgrade the analytics server to a version that understands this data store.",
                "Alternatively, restore a backup created with a compatible version.",
            ],
        ));
    }

    // current < SCHEMA_VERSION (a fresh database reports 0): apply each step in turn.
    let mut version = current;
    while version < SCHEMA_VERSION {
        apply(db, version + 1)?;
        version += 1;
        write_version(db, version)?;
    }
    Ok(())
}

/// Apply the migration that brings the store *to* `version`.
fn apply(_db: &Database, version: u32) -> Result<()> {
    match version {
        // v1 is the initial schema; there is nothing to migrate from an empty store
        // beyond stamping the version (handled by the caller).
        1 => Ok(()),
        // Future migrations slot in here, e.g. `2 => migrate_v2(_db),`.
        other => Err(human_errors::system(
            format!("No migration is defined for schema version {other}."),
            &["This is a bug; please report it with the server version."],
        )),
    }
}

fn read_version(db: &Database) -> Result<u32> {
    let txn = db.begin_read().or_system_err(OPEN_ADVICE)?;
    let table = txn.open_table(META).or_system_err(OPEN_ADVICE)?;
    match table
        .get(META_SCHEMA_VERSION)
        .or_system_err(STORAGE_ADVICE)?
    {
        Some(value) => Ok(u32_from_be(value.value())),
        None => Ok(0),
    }
}

fn write_version(db: &Database, version: u32) -> Result<()> {
    let txn = db.begin_write().or_system_err(STORAGE_ADVICE)?;
    {
        let mut table = txn.open_table(META).or_system_err(STORAGE_ADVICE)?;
        table
            .insert(META_SCHEMA_VERSION, version.to_be_bytes().as_slice())
            .or_system_err(STORAGE_ADVICE)?;
    }
    txn.commit().or_system_err(STORAGE_ADVICE)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_db() -> (Database, std::path::PathBuf) {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "analytics-schema-{}-{}.redb",
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::create(&path).unwrap();
        // The meta table must exist before version reads/writes.
        let txn = db.begin_write().unwrap();
        txn.open_table(META).unwrap();
        txn.commit().unwrap();
        (db, path)
    }

    #[test]
    fn fresh_database_is_stamped_to_current() {
        let (db, path) = temp_db();
        assert_eq!(read_version(&db).unwrap(), 0);
        migrate(&db).unwrap();
        assert_eq!(read_version(&db).unwrap(), SCHEMA_VERSION);
        // Idempotent.
        migrate(&db).unwrap();
        assert_eq!(read_version(&db).unwrap(), SCHEMA_VERSION);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_newer_schema() {
        let (db, path) = temp_db();
        write_version(&db, SCHEMA_VERSION + 1).unwrap();
        let err = migrate(&db).unwrap_err();
        assert!(err.to_string().contains("schema"));
        drop(db);
        let _ = std::fs::remove_file(&path);
    }
}
