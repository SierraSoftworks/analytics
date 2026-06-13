//! Turn a beacon payload + request headers into an enriched, anonymized
//! [`StoredEvent`], dropping bots. Raw IP/User-Agent/Accept-Language never appear
//! in the result — only derived classes.

use std::collections::BTreeMap;

use analytics_api::{BeaconKind, TrackEvent, website_source};
use url::Url;

use super::{geo, language, referrer, ua};
use crate::store::{EventKind, StoredEvent};

/// Build an enriched event from a beacon payload. Returns `None` when the event
/// should be dropped (bot, or an unparseable/host-less URL).
pub fn build_event(
    track: TrackEvent,
    user_agent: &str,
    accept_language: Option<&str>,
    received_ms: i64,
) -> Option<StoredEvent> {
    let url = Url::parse(&track.url).ok()?;
    let hostname = url.host_str()?.trim_start_matches("www.").to_lowercase();
    if hostname.is_empty() {
        return None;
    }

    let ua = ua::classify(user_agent);
    // Bot, or a UA with no recognisable browser/OS/device — drop it.
    if ua.is_bot || (ua.browser.is_none() && ua.os.is_none() && ua.device.is_none()) {
        return None;
    }

    let referrer = referrer::classify(track.referrer.as_deref(), &hostname);
    let language = accept_language.and_then(language::primary_language);
    let country = track
        .timezone
        .as_deref()
        .and_then(geo::country_from_timezone)
        .map(str::to_string);
    let (utm_source, utm_medium, utm_campaign) = extract_utm(&url);

    let kind = match track.kind {
        BeaconKind::Load => EventKind::PageLoad,
        BeaconKind::Unload => EventKind::PageUnload,
        BeaconKind::Custom => EventKind::Custom,
    };

    Some(StoredEvent {
        created_ms: received_ms,
        received_ms,
        bid: track.beacon,
        kind,
        source: website_source(&hostname),
        pathname: Some(normalize_path(url.path())),
        is_unique_user: track.unique_visit,
        is_unique_page: track.unique_page,
        referrer_host: referrer.host,
        referrer_group: referrer.group,
        country,
        language,
        ua_browser: ua.browser,
        ua_os: ua.os,
        ua_device: ua.device,
        utm_source,
        utm_medium,
        utm_campaign,
        duration_ms: track.duration_ms,
        event_name: track.event_name,
        metadata_json: track.metadata.as_ref().and_then(serialize_metadata),
        ..Default::default()
    })
}

fn extract_utm(url: &Url) -> (Option<String>, Option<String>, Option<String>) {
    let (mut source, mut medium, mut campaign) = (None, None, None);
    for (key, value) in url.query_pairs() {
        let value = value.trim().to_string();
        if value.is_empty() {
            continue;
        }
        match key.as_ref() {
            "utm_source" => source = Some(value),
            "utm_medium" => medium = Some(value),
            "utm_campaign" => campaign = Some(value),
            _ => {}
        }
    }
    (source, medium, campaign)
}

/// Normalize a path: keep case, drop a trailing slash, guarantee a leading one.
fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn serialize_metadata(meta: &BTreeMap<String, String>) -> Option<String> {
    if meta.is_empty() {
        None
    } else {
        serde_json::to_string(meta).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chrome() -> &'static str {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
         (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
    }

    fn base(url: &str) -> TrackEvent {
        TrackEvent {
            beacon: "b1".into(),
            kind: BeaconKind::Load,
            url: url.into(),
            referrer: None,
            unique_visit: true,
            unique_page: true,
            timezone: Some("America/New_York".into()),
            duration_ms: None,
            event_name: None,
            metadata: None,
        }
    }

    #[test]
    fn enriches_a_pageview() {
        let e = build_event(
            base("https://www.example.com/Blog/Post?utm_source=news&x=1"),
            chrome(),
            Some("en-US,en;q=0.9"),
            1000,
        )
        .expect("event");
        assert_eq!(e.source, "https://example.com");
        assert_eq!(e.pathname.as_deref(), Some("/Blog/Post"));
        assert_eq!(e.country.as_deref(), Some("US"));
        assert_eq!(e.language.as_deref(), Some("en"));
        assert_eq!(e.utm_source.as_deref(), Some("news"));
        assert_eq!(e.ua_browser.as_deref(), Some("Chrome"));
        assert!(e.is_unique_user);
    }

    #[test]
    fn drops_bots_and_bad_urls() {
        assert!(build_event(base("https://example.com/"), "Googlebot/2.1", None, 1).is_none());
        assert!(build_event(base("not a url"), chrome(), None, 1).is_none());
    }
}
