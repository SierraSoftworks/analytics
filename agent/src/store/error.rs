/// Errors produced by the storage layer.
#[derive(Debug)]
pub enum StoreError {
    Db(redb::Error),
    Serde(serde_json::Error),
    Polars(polars::prelude::PolarsError),
    Io(std::io::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "storage error: {e}"),
            StoreError::Serde(e) => write!(f, "serialization error: {e}"),
            StoreError::Polars(e) => write!(f, "analytics engine error: {e}"),
            StoreError::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

macro_rules! from_redb {
    ($($t:ty),* $(,)?) => {
        $(impl From<$t> for StoreError {
            fn from(e: $t) -> Self { StoreError::Db(e.into()) }
        })*
    };
}

from_redb!(
    redb::Error,
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError,
);

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Serde(e)
    }
}

impl From<polars::prelude::PolarsError> for StoreError {
    fn from(e: polars::prelude::PolarsError) -> Self {
        StoreError::Polars(e)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}
