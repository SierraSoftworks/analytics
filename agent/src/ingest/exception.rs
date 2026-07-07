//! Build anonymized `Exception` events from reports, with Sentry-style grouping.

use analytics_api::{ExceptionReport, website_source};
use sha2::{Digest, Sha256};
use url::Url;

use super::{truncate, ua};
use crate::store::{EventKind, StoredEvent};

const TOP_FRAMES: usize = 5;
const MAX_MESSAGE: usize = 1_000;
const MAX_STACK: usize = 16_000;
const MAX_APP_FIELD: usize = 120;

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
            for frame in normalized_frames(stack).into_iter().take(TOP_FRAMES) {
                hasher.update(frame.as_bytes());
                hasher.update(b"\n");
            }
        }
        None => hasher.update(normalize_noise(message).as_bytes()),
    }

    short_hash(&hasher.finalize())
}

/// First 8 bytes of (a hash of) the input, as 16 hex chars.
fn short_hash(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Normalize stack frames so line/column numbers and addresses don't fragment
/// groups: drop query strings, then strip digits/hex noise, keeping function and
/// file identifiers.
fn normalized_frames(stack: &str) -> Vec<String> {
    stack
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|line| {
            let without_query = line.split('?').next().unwrap_or(line);
            normalize_noise(without_query)
        })
        .filter(|l| !l.is_empty())
        .collect()
}

/// Lower-case, drop digits and `0x` hex markers, and collapse whitespace.
fn normalize_noise(input: &str) -> String {
    let lowered = input.to_lowercase().replace("0x", "");
    let mut out = String::with_capacity(lowered.len());
    let mut last_space = false;
    for ch in lowered.chars() {
        if ch.is_ascii_digit() {
            continue;
        }
        if ch.is_whitespace() {
            if !last_space && !out.is_empty() {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    out.trim().to_string()
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
}
