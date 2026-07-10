//! redb table definitions, key encoding, and shared advice for storage errors.

use redb::TableDefinition;

/// JSON-valued, string-keyed table (projects, sources, pixels, triage, meta).
pub(super) type JsonTable = TableDefinition<'static, &'static str, &'static [u8]>;

/// Append-only event log, keyed by `(received_ms, monotonic_seq)` (16 bytes BE).
pub(super) const EVENTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("events");
pub(super) const PROJECTS: JsonTable = TableDefinition::new("projects");
pub(super) const SOURCES: JsonTable = TableDefinition::new("sources");
pub(super) const PIXELS: JsonTable = TableDefinition::new("pixels");
pub(super) const EXCEPTION_TRIAGE: JsonTable = TableDefinition::new("exception_triage");
pub(super) const META: JsonTable = TableDefinition::new("meta");

pub(super) const META_NEXT_SEQ: &str = "next_seq";
pub(super) const META_SCHEMA_VERSION: &str = "schema_version";
/// The exception grouping-rules version last applied to the stored data. Bumping
/// the code's version (see `ingest::exception::FINGERPRINT_VERSION`) triggers a
/// one-time re-grouping pass on next start.
pub(super) const META_FINGERPRINT_VERSION: &str = "fingerprint_version";

pub(super) const STORAGE_ADVICE: &[&str] = &[
    "This is an internal storage error.",
    "Retry the operation, and if it persists report it with the server logs.",
];
pub(super) const OPEN_ADVICE: &[&str] = &[
    "Ensure the data directory exists and is writable.",
    "Make sure no other analytics process has the database open.",
];

/// 16-byte big-endian event key: `received_ms` then `seq`, so a byte-ordered scan
/// yields events in time order.
pub(super) fn event_key(received_ms: i64, seq: u64) -> [u8; 16] {
    let mut key = [0u8; 16];
    key[0..8].copy_from_slice(&(received_ms.max(0) as u64).to_be_bytes());
    key[8..16].copy_from_slice(&seq.to_be_bytes());
    key
}

pub(super) fn u64_from_be(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let n = bytes.len().min(8);
    buf[8 - n..].copy_from_slice(&bytes[..n]);
    u64::from_be_bytes(buf)
}

pub(super) fn u32_from_be(bytes: &[u8]) -> u32 {
    let mut buf = [0u8; 4];
    let n = bytes.len().min(4);
    buf[4 - n..].copy_from_slice(&bytes[..n]);
    u32::from_be_bytes(buf)
}

/// Composite key for the triage table. The unit-separator byte cannot appear in
/// ULIDs or hostnames, so it is a safe delimiter.
pub(super) fn triage_key(project_id: &str, group_id: &str) -> String {
    format!("{project_id}\u{1f}{group_id}")
}
