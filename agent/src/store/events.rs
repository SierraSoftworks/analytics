//! Append-only event log: ingest via DuckDB's appender, plus row mapping for
//! the paths that read whole events back (tests, the session-trace view, and
//! the legacy importer).

use std::sync::atomic::Ordering;

use duckdb::{Appender, Row, params};

use super::{STORAGE_ADVICE, Store, StoredEvent, event::EventKind};
use crate::errors::{Result, ResultExt};

impl Store {
    /// Append a batch of events in one appender flush (a single WAL'd
    /// transaction). Non-blocking ingest is achieved by the caller feeding this
    /// from a background writer task. Each event is stamped with its monotonic
    /// `seq` before being persisted.
    pub fn append_events(&self, events: &[StoredEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        self.with_conn(|conn| {
            let mut appender = conn.appender("events").or_system_err(STORAGE_ADVICE)?;
            for event in events {
                let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
                append_row(&mut appender, event, seq)?;
            }
            appender.flush().or_system_err(STORAGE_ADVICE)
        })
    }

    /// Append already-stamped events, preserving their `seq` (the legacy
    /// importer). The caller refreshes the sequence counter afterwards.
    pub(super) fn import_events(&self, events: &[StoredEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        self.with_conn(|conn| {
            let mut appender = conn.appender("events").or_system_err(STORAGE_ADVICE)?;
            for event in events {
                append_row(&mut appender, event, event.seq)?;
            }
            appender.flush().or_system_err(STORAGE_ADVICE)
        })
    }

    /// Every stored event, oldest first (tests and diagnostics; production
    /// queries aggregate in SQL instead of materializing events).
    pub fn all_events(&self) -> Result<Vec<StoredEvent>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT * FROM events ORDER BY received_ms, seq")
                .or_system_err(STORAGE_ADVICE)?;
            let rows = stmt
                .query_map([], event_from_row)
                .or_system_err(STORAGE_ADVICE)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.or_system_err(STORAGE_ADVICE)?);
            }
            Ok(out)
        })
    }

    /// Number of stored events.
    pub fn event_count(&self) -> Result<u64> {
        self.with_conn(|conn| {
            conn.query_row("SELECT count(*) FROM events", [], |row| row.get(0))
                .or_system_err(STORAGE_ADVICE)
        })
    }
}

/// Append one event; the parameter order must match [`super::schema`]'s table
/// definition.
fn append_row(appender: &mut Appender<'_>, event: &StoredEvent, seq: u64) -> Result<()> {
    appender
        .append_row(params![
            event.created_ms,
            event.received_ms,
            seq,
            event.bid,
            event.sid,
            event.kind.as_str(),
            event.source,
            event.pathname,
            event.is_unique_user,
            event.is_unique_page,
            event.referrer_host,
            event.referrer_group,
            event.country,
            event.language,
            event.ua_browser,
            event.ua_version,
            event.ua_os,
            event.ua_device,
            event.utm_source,
            event.utm_medium,
            event.utm_campaign,
            event.duration_ms,
            event.event_name,
            event.metadata_json,
            event.app_version,
            event.exc_type,
            event.exc_message,
            event.exc_stack,
            event.exc_group,
            event.exc_handled,
        ])
        .or_system_err(STORAGE_ADVICE)
}

/// Map a `SELECT *` row (table column order) back into a [`StoredEvent`].
pub(super) fn event_from_row(row: &Row<'_>) -> duckdb::Result<StoredEvent> {
    Ok(StoredEvent {
        created_ms: row.get(0)?,
        received_ms: row.get(1)?,
        seq: row.get(2)?,
        bid: row.get(3)?,
        sid: row.get(4)?,
        kind: EventKind::parse(&row.get::<_, String>(5)?),
        source: row.get(6)?,
        pathname: row.get(7)?,
        is_unique_user: row.get(8)?,
        is_unique_page: row.get(9)?,
        referrer_host: row.get(10)?,
        referrer_group: row.get(11)?,
        country: row.get(12)?,
        language: row.get(13)?,
        ua_browser: row.get(14)?,
        ua_version: row.get(15)?,
        ua_os: row.get(16)?,
        ua_device: row.get(17)?,
        utm_source: row.get(18)?,
        utm_medium: row.get(19)?,
        utm_campaign: row.get(20)?,
        duration_ms: row.get(21)?,
        event_name: row.get(22)?,
        metadata_json: row.get(23)?,
        app_version: row.get(24)?,
        exc_type: row.get(25)?,
        exc_message: row.get(26)?,
        exc_stack: row.get(27)?,
        exc_group: row.get(28)?,
        exc_handled: row.get(29)?,
    })
}
