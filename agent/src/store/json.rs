//! Generic JSON-valued CRUD helpers shared by the entity tables.

use redb::{ReadableDatabase, ReadableTable};
use serde::{Serialize, de::DeserializeOwned};

use super::Store;
use super::tables::{JsonTable, STORAGE_ADVICE};
use crate::errors::{Result, ResultExt};

impl Store {
    pub(super) fn put_json<T: Serialize>(
        &self,
        def: JsonTable,
        key: &str,
        value: &T,
    ) -> Result<()> {
        let bytes = serde_json::to_vec(value).or_system_err(STORAGE_ADVICE)?;
        let txn = self.db.begin_write().or_system_err(STORAGE_ADVICE)?;
        {
            let mut table = txn.open_table(def).or_system_err(STORAGE_ADVICE)?;
            table
                .insert(key, bytes.as_slice())
                .or_system_err(STORAGE_ADVICE)?;
        }
        txn.commit().or_system_err(STORAGE_ADVICE)?;
        Ok(())
    }

    pub(super) fn get_json<T: DeserializeOwned>(
        &self,
        def: JsonTable,
        key: &str,
    ) -> Result<Option<T>> {
        let txn = self.db.begin_read().or_system_err(STORAGE_ADVICE)?;
        let table = txn.open_table(def).or_system_err(STORAGE_ADVICE)?;
        match table.get(key).or_system_err(STORAGE_ADVICE)? {
            Some(value) => Ok(Some(
                serde_json::from_slice(value.value()).or_system_err(STORAGE_ADVICE)?,
            )),
            None => Ok(None),
        }
    }

    pub(super) fn list_json<T: DeserializeOwned>(&self, def: JsonTable) -> Result<Vec<T>> {
        let txn = self.db.begin_read().or_system_err(STORAGE_ADVICE)?;
        let table = txn.open_table(def).or_system_err(STORAGE_ADVICE)?;
        let mut out = Vec::new();
        for item in table.iter().or_system_err(STORAGE_ADVICE)? {
            let (_key, value) = item.or_system_err(STORAGE_ADVICE)?;
            out.push(serde_json::from_slice(value.value()).or_system_err(STORAGE_ADVICE)?);
        }
        Ok(out)
    }

    pub(super) fn delete_key(&self, def: JsonTable, key: &str) -> Result<bool> {
        let txn = self.db.begin_write().or_system_err(STORAGE_ADVICE)?;
        let existed = {
            let mut table = txn.open_table(def).or_system_err(STORAGE_ADVICE)?;
            table.remove(key).or_system_err(STORAGE_ADVICE)?.is_some()
        };
        txn.commit().or_system_err(STORAGE_ADVICE)?;
        Ok(existed)
    }
}
