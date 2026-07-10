//! Build anonymized `Exception` events from reports, with Sentry-style grouping.

use analytics_api::{ExceptionReport, summary_line, website_source};
use sha2::{Digest, Sha256};
use url::Url;

use super::{normalize, truncate, ua};
use crate::store::{EventKind, StoredEvent};

const TOP_FRAMES: usize = 5;
const MAX_MESSAGE: usize = 1_000;
const MAX_STACK: usize = 16_000;
const MAX_APP_FIELD: usize = 120;

/// The version of the exception grouping rules — the fingerprint logic in this
/// module together with the `normalize` pipeline. Bump this whenever a change would
/// assign existing exceptions to different groups; on next start the store detects
/// the mismatch and re-groups its stored occurrences (see
/// `ingest::regroup_if_needed`). A store predating this marker reports `0`, so the
/// initial value of `1` re-groups it once under the current aggressive rules.
pub const FINGERPRINT_VERSION: u32 = 1;

/// Build an `Exception` event from a report. Returns `None` for bots or an
/// unparseable URL (we attribute exceptions to a source by hostname).
pub fn build_exception(
    report: ExceptionReport,
    user_agent: &str,
    received_ms: i64,
) -> Option<StoredEvent> {
    let url = Url::parse(&report.url).ok()?;
    let hostname = url.host_str()?.trim_start_matches("www.").to_lowercase();
    if hostname.is_empty() {
        return None;
    }

    let ua = ua::classify(user_agent);
    if ua.kind == ua::UaKind::Bot {
        return None;
    }

    let group = fingerprint(
        &report.exc_type,
        &report.message,
        report.stack.as_deref(),
        report.fingerprint.as_deref(),
    );

    Some(StoredEvent {
        created_ms: received_ms,
        received_ms,
        bid: report.beacon.unwrap_or_default(),
        sid: super::enrich::clean_session(report.session.as_deref()),
        kind: EventKind::Exception,
        source: website_source(&hostname),
        is_unique_user: false,
        is_unique_page: false,
        ua_browser: ua.app,
        ua_version: ua.version,
        ua_os: ua.os,
        ua_device: Some(ua.kind.as_str().to_string()),
        metadata_json: report
            .metadata
            .as_ref()
            .filter(|m| !m.is_empty())
            .and_then(|m| serde_json::to_string(m).ok()),
        app_version: clean_app_field(report.app_version.as_deref()),
        exc_type: Some(truncate(&report.exc_type, MAX_MESSAGE)),
        exc_message: Some(truncate(&report.message, MAX_MESSAGE)),
        exc_stack: report.stack.map(|s| truncate(&s, MAX_STACK)),
        exc_group: Some(group),
        exc_handled: Some(report.handled),
        ..Default::default()
    })
}

/// Trim and cap a client-reported app name/version, dropping empty values.
fn clean_app_field(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| truncate(v, MAX_APP_FIELD))
}

/// Compute a stable grouping fingerprint: a client override if given, otherwise a
/// hash of the type + normalized top stack frames (falling back to the normalized
/// message when there is no stack).
pub fn fingerprint(
    exc_type: &str,
    message: &str,
    stack: Option<&str>,
    override_fp: Option<&str>,
) -> String {
    if let Some(fp) = override_fp.map(str::trim).filter(|f| !f.is_empty()) {
        return short_hash(fp.as_bytes());
    }

    let mut hasher = Sha256::new();
    hasher.update(exc_type.trim().to_lowercase().as_bytes());
    hasher.update(b"\n");

    match stack.map(str::trim).filter(|s| !s.is_empty()) {
        Some(stack) => {
            for frame in normalize::frames(stack).into_iter().take(TOP_FRAMES) {
                hasher.update(frame.as_bytes());
                hasher.update(b"\n");
            }
        }
        // Without a stack, group on the message's summary line only: the lines
        // of context that follow it vary between otherwise-identical failures
        // and would needlessly fragment the group.
        None => hasher.update(normalize::message(summary_line(message)).as_bytes()),
    }

    short_hash(&hasher.finalize())
}

/// First 8 bytes of (a hash of) the input, as 16 hex chars.
fn short_hash(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_error_different_line_numbers_groups_together() {
        let a = fingerprint(
            "TypeError",
            "x is undefined",
            Some("at handler (app.js:42:10)\nat main (app.js:99:3)"),
            None,
        );
        let b = fingerprint(
            "TypeError",
            "x is undefined",
            Some("at handler (app.js:43:18)\nat main (app.js:120:7)"),
            None,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn different_types_group_apart() {
        let a = fingerprint("TypeError", "boom", None, None);
        let b = fingerprint("RangeError", "boom", None, None);
        assert_ne!(a, b);
    }

    #[test]
    fn client_override_is_respected() {
        let a = fingerprint("TypeError", "a", Some("frame1"), Some("custom"));
        let b = fingerprint("RangeError", "b", Some("frame2"), Some("custom"));
        assert_eq!(a, b);
    }

    #[test]
    fn message_only_fallback_ignores_numbers() {
        let a = fingerprint("Error", "HTTP 404 from /api", None, None);
        let b = fingerprint("Error", "HTTP 500 from /api", None, None);
        assert_eq!(a, b);
    }

    #[test]
    fn multiline_messages_group_on_summary_line() {
        // Same summary line, different trailing context (line/column numbers,
        // varying diagnostic detail) must land in the same group.
        let a = fingerprint(
            "ConfigError",
            "Failed to parse configuration file.\n\nCaused by:\n - filter at line 1, column 19",
            None,
            None,
        );
        let b = fingerprint(
            "ConfigError",
            "Failed to parse configuration file.\n\nCaused by:\n - filter at line 8, column 42\n - make sure brackets are closed",
            None,
            None,
        );
        assert_eq!(a, b);
    }

    // The stack a `human_errors`/`tracing_batteries` client reports leads with the
    // "caused by" chain, then the (generic) backtrace. Only the request URL varies
    // between these occurrences, so they must land in a single group.
    fn transport_stack(url: &str) -> String {
        format!(
            "caused by: error sending request for url ({url})\n\
             caused by: client error (SendRequest)\n\
             caused by: connection error\n\
             caused by: connection reset\n\
             \n\
             0: tracing_batteries::error_info::ErrorInfo::new\n\
             1: <github_backup::LoggingPairingHandler as github_backup::pairing::PairingHandler<E>>::on_error\n\
             2: github_backup::main"
        )
    }

    #[test]
    fn same_transport_failure_different_urls_group_together() {
        let a = fingerprint(
            "human_errors::error::Error",
            "System failure",
            Some(&transport_stack(
                "https://git.raptor-perch.ts.net/api/v1/repos/sierrasoftworks/update-go/releases/tags/v1.1.0",
            )),
            None,
        );
        let b = fingerprint(
            "human_errors::error::Error",
            "System failure",
            Some(&transport_stack(
                "https://git.other-host.ts.net/api/v1/repos/sierrasoftworks/backup/releases/tags/v9.9.9",
            )),
            None,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn distinct_failure_shapes_stay_apart() {
        // A different underlying cause is a genuinely different failure and must
        // not be folded in just because the URL was stripped.
        let reset = fingerprint(
            "human_errors::error::Error",
            "System failure",
            Some(&transport_stack("https://host/a")),
            None,
        );
        let dns = fingerprint(
            "human_errors::error::Error",
            "System failure",
            Some(
                &"caused by: error sending request for url (https://host/a)\n\
                  caused by: client error (SendRequest)\n\
                  caused by: dns error\n\
                  0: github_backup::main"
                    .to_string(),
            ),
            None,
        );
        assert_ne!(reset, dns);
    }

    #[test]
    fn hashed_bundle_urls_do_not_fragment_browser_frames() {
        // Cache-busted bundle filenames change every deploy; grouping should key
        // on the (stable) function name, not the volatile URL.
        let a = fingerprint(
            "TypeError",
            "x is undefined",
            Some("at render (https://cdn.example.com/static/app.abc123.js:10:5)"),
            None,
        );
        let b = fingerprint(
            "TypeError",
            "x is undefined",
            Some("at render (https://cdn.example.com/static/app.def456.js:88:3)"),
            None,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn urls_are_stripped_from_the_message_fallback() {
        // With no stack, the message summary still must not fragment on the URL.
        let a = fingerprint(
            "Error",
            "error sending request for url (https://a.example.com/x/v1.1.0)",
            None,
            None,
        );
        let b = fingerprint(
            "Error",
            "error sending request for url (https://b.other.net/y/v2.0.0)",
            None,
            None,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn recursive_overflow_groups_regardless_of_depth() {
        // A stack overflow reports the same recursive frame a variable number of
        // times; de-recursion must keep those occurrences in one group.
        let shallow = fingerprint(
            "RangeError",
            "Maximum call stack size exceeded",
            Some(&format!("{}at main (index.js:1:1)", "at walk (tree.js:9:3)\n".repeat(3))),
            None,
        );
        let deep = fingerprint(
            "RangeError",
            "Maximum call stack size exceeded",
            Some(&format!("{}at main (index.js:1:1)", "at walk (tree.js:9:3)\n".repeat(400))),
            None,
        );
        assert_eq!(shallow, deep);
    }

    #[test]
    fn request_ids_do_not_fragment_groups() {
        // The same failure carrying a different request id (UUID) must group.
        let a = fingerprint(
            "human_errors::error::Error",
            "request 550e8400-e29b-41d4-a716-446655440000 failed",
            None,
            None,
        );
        let b = fingerprint(
            "human_errors::error::Error",
            "request 7c9e6679-7425-40de-944b-e07fc1f90ae7 failed",
            None,
            None,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn build_directory_does_not_fragment_groups() {
        // The same crash built/run from different directories (a dev machine vs a
        // CI checkout vs a container) must land in one group.
        let dev = fingerprint(
            "panic",
            "index out of bounds",
            Some("at bender::store::read (/home/alice/bender/src/store.rs:88:14)\nat bender::main (/home/alice/bender/src/main.rs:24:5)"),
            None,
        );
        let ci = fingerprint(
            "panic",
            "index out of bounds",
            Some("at bender::store::read (/build/12345/bender/src/store.rs:88:14)\nat bender::main (/build/12345/bender/src/main.rs:24:5)"),
            None,
        );
        assert_eq!(dev, ci);
    }
}

