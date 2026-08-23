//! DuckDB schema and instance configuration.
//!
//! The `events` table is typed per the DuckDB schema guidance: `NOT NULL`
//! wherever the ingest pipeline guarantees a value, and rows are appended in
//! arrival (≈ time) order so zone maps prune `received_ms` ranges without an
//! index. Low-cardinality dimensions stay `VARCHAR` rather than `ENUM`
//! deliberately: DuckDB ENUMs cannot grow after creation, and every dimension
//! except `kind` is an open set (new countries, browsers, devices appear at
//! ingest time); row-group dictionary encoding gives most of the ENUM benefit
//! without the migration hazard.
//!
//! The metadata entities (projects, sources, pixels, triage, meta) are
//! low-volume key→JSON tables, preserving the exact serde representation they
//! had in redb — serde stays the single source of truth for their shape, and
//! adding a field never needs a schema migration.

use duckdb::Connection;

use super::OPEN_ADVICE;
use crate::config::StorageConfig;
use crate::errors::{Result, ResultExt};

/// Every column of the `events` table, in table order. Shared by the appender,
/// the row mapper, and the legacy-archive importer, which must all agree.
pub(super) const EVENT_COLUMNS: &[&str] = &[
    "created_ms",
    "received_ms",
    "seq",
    "bid",
    "sid",
    "kind",
    "source",
    "pathname",
    "is_unique_user",
    "is_unique_page",
    "referrer_host",
    "referrer_group",
    "country",
    "language",
    "ua_browser",
    "ua_version",
    "ua_os",
    "ua_device",
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "duration_ms",
    "event_name",
    "metadata_json",
    "app_version",
    "exc_type",
    "exc_message",
    "exc_stack",
    "exc_group",
    "exc_handled",
];

const DDL: &str = "
CREATE TABLE IF NOT EXISTS events (
    created_ms     BIGINT  NOT NULL,
    received_ms    BIGINT  NOT NULL,
    seq            UBIGINT NOT NULL,
    bid            VARCHAR NOT NULL,
    sid            VARCHAR,
    kind           VARCHAR NOT NULL,
    source         VARCHAR NOT NULL,
    pathname       VARCHAR,
    is_unique_user BOOLEAN NOT NULL,
    is_unique_page BOOLEAN NOT NULL,
    referrer_host  VARCHAR,
    referrer_group VARCHAR,
    country        VARCHAR,
    language       VARCHAR,
    ua_browser     VARCHAR,
    ua_version     VARCHAR,
    ua_os          VARCHAR,
    ua_device      VARCHAR,
    utm_source     VARCHAR,
    utm_medium     VARCHAR,
    utm_campaign   VARCHAR,
    duration_ms    BIGINT,
    event_name     VARCHAR,
    metadata_json  VARCHAR,
    app_version    VARCHAR,
    exc_type       VARCHAR,
    exc_message    VARCHAR,
    exc_stack      VARCHAR,
    exc_group      VARCHAR,
    exc_handled    BOOLEAN
);
CREATE TABLE IF NOT EXISTS projects         (key VARCHAR PRIMARY KEY, data VARCHAR NOT NULL);
CREATE TABLE IF NOT EXISTS sources          (key VARCHAR PRIMARY KEY, data VARCHAR NOT NULL);
CREATE TABLE IF NOT EXISTS pixels           (key VARCHAR PRIMARY KEY, data VARCHAR NOT NULL);
CREATE TABLE IF NOT EXISTS exception_triage (key VARCHAR PRIMARY KEY, data VARCHAR NOT NULL);
CREATE TABLE IF NOT EXISTS meta             (key VARCHAR PRIMARY KEY, data VARCHAR NOT NULL);
";

/// Create every table this build expects (idempotent).
pub(super) fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(DDL).or_system_err(OPEN_ADVICE)
}

/// Apply instance-level settings from the config. `memory_limit` bounds the
/// buffer manager for the whole database instance (shared by every pooled
/// connection), with larger operations spilling to disk instead of growing RSS.
pub(super) fn configure(conn: &Connection, storage: &StorageConfig) -> Result<()> {
    conn.execute_batch(&format!(
        "SET memory_limit = '{}MB';",
        storage.memory_limit_mb.max(64)
    ))
    .or_system_err(OPEN_ADVICE)
}
