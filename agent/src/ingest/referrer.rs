//! Classify a referrer URL into a host and a coarse group (Search/Social), with
//! self-referrals dropped.

use url::Url;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Referrer {
    pub host: Option<String>,
    pub group: Option<String>,
}

const SEARCH: &[&str] = &[
    "google.", "bing.", "duckduckgo.", "yahoo.", "yandex.", "baidu.", "ecosia.", "brave.",
    "startpage.", "qwant.",
];
const SOCIAL: &[&str] = &[
    "facebook.", "twitter.", "x.com", "t.co", "linkedin.", "reddit.", "instagram.", "youtube.",
    "mastodon", "bsky.", "pinterest.", "tiktok.", "news.ycombinator.com", "lobste.rs", "threads.",
];

/// Resolve a referrer URL to a host + group, treating same-host referrals as
/// internal (no referrer).
pub fn classify(referrer: Option<&str>, self_host: &str) -> Referrer {
    let Some(raw) = referrer.map(str::trim).filter(|r| !r.is_empty()) else {
        return Referrer::default();
    };

    let host = match Url::parse(raw).ok().and_then(|u| u.host_str().map(normalize)) {
        Some(h) => h,
        None => return Referrer::default(),
    };

    if host.eq_ignore_ascii_case(self_host) {
        return Referrer::default();
    }

    let group = group_for(&host);
    Referrer {
        host: Some(host),
        group,
    }
}

fn normalize(host: &str) -> String {
    host.trim_start_matches("www.").to_lowercase()
}

fn group_for(host: &str) -> Option<String> {
    if SEARCH.iter().any(|p| host.contains(p)) {
        Some("Search".to_string())
    } else if SOCIAL.iter().any(|p| host.contains(p)) {
        Some("Social".to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_search_and_social() {
        assert_eq!(
            classify(Some("https://www.google.com/search?q=x"), "example.com"),
            Referrer {
                host: Some("google.com".into()),
                group: Some("Search".into())
            }
        );
        assert_eq!(
            classify(Some("https://t.co/abc"), "example.com").group.as_deref(),
            Some("Social")
        );
    }

    #[test]
    fn drops_self_and_empty_referrals() {
        assert_eq!(
            classify(Some("https://example.com/page"), "example.com"),
            Referrer::default()
        );
        assert_eq!(classify(None, "example.com"), Referrer::default());
        assert_eq!(classify(Some(""), "example.com"), Referrer::default());
    }

    #[test]
    fn plain_referral_has_host_without_group() {
        let r = classify(Some("https://blog.somesite.io/post"), "example.com");
        assert_eq!(r.host.as_deref(), Some("blog.somesite.io"));
        assert_eq!(r.group, None);
    }
}
