//! On-disk schema versioning and forward migrations.
//!
//! The current version is stamped in the `meta` table. On open, [`migrate`] applies
//! any pending migration steps in order. A database newer than this build is
//! rejected rather than silently misread.

use chrono::{DateTime, Utc};
use redb::{Database, ReadableDatabase, ReadableTable};
use serde::Deserialize;

use super::tables::{
    EXCEPTION_TRIAGE, META, META_SCHEMA_VERSION, OPEN_ADVICE, STORAGE_ADVICE, u32_from_be,
};
use super::triage::ExceptionTriage;
use crate::errors::{Result, ResultExt};

/// The current on-disk schema version. Bump this and add an [`apply`] arm whenever
/// the stored layout changes incompatibly.
pub(super) const SCHEMA_VERSION: u32 = 2;

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
fn apply(db: &Database, version: u32) -> Result<()> {
    match version {
        // v1 is the initial schema; there is nothing to migrate from an empty store
        // beyond stamping the version (handled by the caller).
        1 => Ok(()),
        // v2 splits the single exception-triage `status` into two independent
        // axes: a `resolved_at` anchor (the sole source of truth for resolution,
        // so a later occurrence reopens the group automatically) and a `muted_at`
        // suppression flag.
        2 => migrate_triage_to_axes(db),
        other => Err(human_errors::system(
            format!("No migration is defined for schema version {other}."),
            &["This is a bug; please report it with the server version."],
        )),
    }
}

/// The pre-v2 triage record: a single collapsed `status`.
#[derive(Deserialize)]
struct LegacyTriage {
    #[serde(default)]
    status: String,
    #[serde(default)]
    note: Option<String>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    updated_by: Option<String>,
}

/// Rewrite every triage record from the collapsed `status` to the resolution +
/// suppression axes. A `resolved` record's `updated_at` was when it was resolved,
/// so it becomes the `resolved_at` anchor; likewise `ignored` becomes `muted_at`.
fn migrate_triage_to_axes(db: &Database) -> Result<()> {
    let txn = db.begin_write().or_system_err(STORAGE_ADVICE)?;
    {
        let mut table = txn
            .open_table(EXCEPTION_TRIAGE)
            .or_system_err(STORAGE_ADVICE)?;
        let mut rewrites: Vec<(String, Vec<u8>)> = Vec::new();
        for item in table.iter().or_system_err(STORAGE_ADVICE)? {
            let (key, value) = item.or_system_err(STORAGE_ADVICE)?;
            let legacy: LegacyTriage =
                serde_json::from_slice(value.value()).or_system_err(STORAGE_ADVICE)?;
            let (resolved_at, muted_at) = match legacy.status.as_str() {
                "resolved" => (Some(legacy.updated_at), None),
                "ignored" => (None, Some(legacy.updated_at)),
                _ => (None, None),
            };
            let migrated = ExceptionTriage {
                resolved_at,
                muted_at,
                note: legacy.note,
                updated_at: legacy.updated_at,
                updated_by: legacy.updated_by,
            };
            let bytes = serde_json::to_vec(&migrated).or_system_err(STORAGE_ADVICE)?;
            rewrites.push((key.value().to_string(), bytes));
        }
        for (key, bytes) in rewrites {
            table
                .insert(key.as_str(), bytes.as_slice())
                .or_system_err(STORAGE_ADVICE)?;
        }
    }
    txn.commit().or_system_err(STORAGE_ADVICE)?;
    Ok(())
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

    #[test]
    fn v2_splits_triage_status_into_axes() {
        use super::super::tables::{EXCEPTION_TRIAGE, triage_key};
        let (db, path) = temp_db();
        // Seed pre-v2 records carrying the collapsed `status`.
        write_version(&db, 1).unwrap();
        let updated_at = Utc::now();
        let legacy = |status: &str| {
            serde_json::json!({
                "status": status,
                "note": "n",
                "updated_at": updated_at,
                "updated_by": "admin",
            })
            .to_string()
        };
        {
            let txn = db.begin_write().unwrap();
            {
                let mut t = txn.open_table(EXCEPTION_TRIAGE).unwrap();
                t.insert(triage_key("p", "res").as_str(), legacy("resolved").as_bytes())
                    .unwrap();
                t.insert(triage_key("p", "ign").as_str(), legacy("ignored").as_bytes())
                    .unwrap();
                t.insert(triage_key("p", "unr").as_str(), legacy("unresolved").as_bytes())
                    .unwrap();
            }
            txn.commit().unwrap();
        }

        migrate(&db).unwrap();
        assert_eq!(read_version(&db).unwrap(), SCHEMA_VERSION);

        let read = |key: String| -> ExceptionTriage {
            let txn = db.begin_read().unwrap();
            let t = txn.open_table(EXCEPTION_TRIAGE).unwrap();
            let v = t.get(key.as_str()).unwrap().unwrap();
            serde_json::from_slice(v.value()).unwrap()
        };

        let res = read(triage_key("p", "res"));
        assert_eq!(res.resolved_at, Some(updated_at), "resolved anchors at updated_at");
        assert!(res.muted_at.is_none());
        assert_eq!(res.note.as_deref(), Some("n"), "note preserved");

        let ign = read(triage_key("p", "ign"));
        assert_eq!(ign.muted_at, Some(updated_at), "ignored becomes muted");
        assert!(ign.resolved_at.is_none());

        let unr = read(triage_key("p", "unr"));
        assert!(unr.resolved_at.is_none() && unr.muted_at.is_none());

        drop(db);
        let _ = std::fs::remove_file(&path);
    }
}
