//! Generic key→JSON CRUD helpers shared by the entity tables. Entities keep
//! their exact serde representation (as they had in redb), so serde stays the
//! single source of truth for their shape and adding a field never needs a
//! schema migration. Table names are compile-time constants from [`entities`],
//! never user input.

use duckdb::params;
use serde::{Serialize, de::DeserializeOwned};

use super::{STORAGE_ADVICE, Store};
use crate::errors::{Result, ResultExt};

impl Store {
    pub(super) fn put_json<T: Serialize>(
        &self,
        table: &'static str,
        key: &str,
        value: &T,
    ) -> Result<()> {
        let data = serde_json::to_string(value).or_system_err(STORAGE_ADVICE)?;
        self.with_conn(|conn| {
            conn.execute(
                &format!("INSERT OR REPLACE INTO {table} VALUES (?, ?)"),
                params![key, data],
            )
            .or_system_err(STORAGE_ADVICE)?;
            Ok(())
        })
    }

    pub(super) fn get_json<T: DeserializeOwned>(
        &self,
        table: &'static str,
        key: &str,
    ) -> Result<Option<T>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(&format!("SELECT data FROM {table} WHERE key = ?"))
                .or_system_err(STORAGE_ADVICE)?;
            let mut rows = stmt.query(params![key]).or_system_err(STORAGE_ADVICE)?;
            match rows.next().or_system_err(STORAGE_ADVICE)? {
                Some(row) => {
                    let data: String = row.get(0).or_system_err(STORAGE_ADVICE)?;
                    Ok(Some(
                        serde_json::from_str(&data).or_system_err(STORAGE_ADVICE)?,
                    ))
                }
                None => Ok(None),
            }
        })
    }

    pub(super) fn list_json<T: DeserializeOwned>(&self, table: &'static str) -> Result<Vec<T>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(&format!("SELECT data FROM {table} ORDER BY key"))
                .or_system_err(STORAGE_ADVICE)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .or_system_err(STORAGE_ADVICE)?;
            let mut out = Vec::new();
            for data in rows {
                let data = data.or_system_err(STORAGE_ADVICE)?;
                out.push(serde_json::from_str(&data).or_system_err(STORAGE_ADVICE)?);
            }
            Ok(out)
        })
    }

    /// Read, mutate, and write back a value in a single transaction, so two
    /// concurrent edits can't clobber each other via a read-modify-write split
    /// across transactions. `f` runs only when the key exists; returns the
    /// updated value, or `None` if it was absent.
    pub(super) fn mutate_json<T, F>(
        &self,
        table: &'static str,
        key: &str,
        f: F,
    ) -> Result<Option<T>>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce(&mut T),
    {
        self.with_conn(|conn| {
            let txn = conn.transaction().or_system_err(STORAGE_ADVICE)?;
            let current: Option<String> = {
                let mut stmt = txn
                    .prepare(&format!("SELECT data FROM {table} WHERE key = ?"))
                    .or_system_err(STORAGE_ADVICE)?;
                let mut rows = stmt.query(params![key]).or_system_err(STORAGE_ADVICE)?;
                match rows.next().or_system_err(STORAGE_ADVICE)? {
                    Some(row) => Some(row.get(0).or_system_err(STORAGE_ADVICE)?),
                    None => None,
                }
            };
            let updated = match current {
                Some(data) => {
                    let mut value: T = serde_json::from_str(&data).or_system_err(STORAGE_ADVICE)?;
                    f(&mut value);
                    let data = serde_json::to_string(&value).or_system_err(STORAGE_ADVICE)?;
                    txn.execute(
                        &format!("INSERT OR REPLACE INTO {table} VALUES (?, ?)"),
                        params![key, data],
                    )
                    .or_system_err(STORAGE_ADVICE)?;
                    Some(value)
                }
                None => None,
            };
            txn.commit().or_system_err(STORAGE_ADVICE)?;
            Ok(updated)
        })
    }

    pub(super) fn delete_key(&self, table: &'static str, key: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let deleted = conn
                .execute(&format!("DELETE FROM {table} WHERE key = ?"), params![key])
                .or_system_err(STORAGE_ADVICE)?;
            Ok(deleted > 0)
        })
    }
}
