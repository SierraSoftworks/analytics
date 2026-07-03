//! User-agent classification. The raw UA string is parsed into broad classes
//! (app, version, OS) and a device kind; the raw string is never stored.
//!
//! Browsers and known crawlers are classified by woothee. Anything woothee
//! cannot identify — pure application user agents like `curl/8.5.0` or
//! `MyApp/2.4.1 (Windows NT 10.0)` — falls back to a product-token parser
//! rather than being treated as a bot.

use woothee::parser::Parser;

/// What kind of client sent the request. Browsers split into desktop/mobile by
/// form factor; anything that identifies itself as a program rather than a
/// browser is an app; bots are dropped at ingest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UaKind {
    Desktop,
    Mobile,
    App,
    #[default]
    Bot,
}

impl UaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            UaKind::Desktop => "Desktop",
            UaKind::Mobile => "Mobile",
            UaKind::App => "App",
            UaKind::Bot => "Bot",
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct UaClasses {
    /// The client program: the browser name, or for applications the first
    /// non-version product segment of the UA string.
    pub app: Option<String>,
    /// The client's version: the browser version, or for applications the
    /// version attached to the product segment (falling back to the first
    /// `\d+(\.\d+)+` match outside parenthesized platform details).
    pub version: Option<String>,
    /// OS family, normalized to Windows / macOS / Linux / Android / iOS /
    /// ChromeOS where recognized.
    pub os: Option<String>,
    pub kind: UaKind,
}

/// Classify a User-Agent header value into non-identifying classes.
pub fn classify(user_agent: &str) -> UaClasses {
    let agent = user_agent.trim();
    if agent.is_empty() {
        return UaClasses::default(); // kind: Bot — dropped upstream
    }

    let parsed = Parser::new().parse(agent);

    // Woothee identified the client itself: trust its name/version/category.
    // Its catch-all labels ("HTTP Library" for curl/wget/okhttp/…, "RSSReader")
    // are not identifications — those fall through to the product-token parser
    // so the actual application name is kept.
    const GENERIC_LABELS: &[&str] = &["HTTP Library", "RSSReader"];
    if let Some(result) = &parsed
        && let Some(name) = clean(result.name)
        && !GENERIC_LABELS.contains(&name.as_str())
    {
        let kind = if is_bot_name(&name) || result.category.eq_ignore_ascii_case("crawler") {
            UaKind::Bot
        } else {
            match result.category {
                "smartphone" | "mobilephone" => UaKind::Mobile,
                // "appliance" is TV/console browsers; closest of our kinds.
                "pc" | "appliance" => UaKind::Desktop,
                // "misc" is desktop tools (feed readers and the like).
                _ => UaKind::App,
            }
        };
        return UaClasses {
            app: Some(name),
            version: clean(result.version),
            os: clean(result.os).and_then(|os| normalize_os(&os)),
            kind,
        };
    }

    // Unrecognized client (a pure application, or an unknown browser): extract
    // the product token ourselves. Woothee may still have recognized the OS
    // (it matches OS platform details independently of the client name) and,
    // for its catch-all labels, a version.
    let os = parsed
        .as_ref()
        .and_then(|r| clean(r.os))
        .and_then(|os| normalize_os(&os))
        .or_else(|| sniff_os(agent));
    let (app, version) = product_token(agent);
    let version = version.or_else(|| parsed.as_ref().and_then(|r| clean(r.version)));

    let is_crawler = parsed
        .as_ref()
        .is_some_and(|r| r.category.eq_ignore_ascii_case("crawler"));
    let kind = match &app {
        _ if is_crawler => UaKind::Bot,
        Some(name) if is_bot_name(name) => UaKind::Bot,
        // No product and no platform: nothing recognizable, treat as a bot.
        None if os.is_none() => UaKind::Bot,
        _ => UaKind::App,
    };
    UaClasses {
        app,
        version,
        os,
        kind,
    }
}

fn clean(value: &str) -> Option<String> {
    if value.is_empty() || value.eq_ignore_ascii_case("UNKNOWN") {
        None
    } else {
        Some(value.to_string())
    }
}

fn is_bot_name(name: &str) -> bool {
    let name = name.to_lowercase();
    ["bot", "crawler", "spider"]
        .iter()
        .any(|marker| name.contains(marker))
}

/// Fold an OS label (woothee's, e.g. "Windows 10" / "Mac OSX" / "iPhone") into
/// a small set of families. Unrecognized labels pass through unchanged.
fn normalize_os(os: &str) -> Option<String> {
    let lower = os.to_lowercase();
    let family = if lower.starts_with("windows") {
        "Windows"
    } else if lower.starts_with("iphone")
        || lower.starts_with("ipad")
        || lower.starts_with("ipod")
        || lower.starts_with("ios")
    {
        "iOS"
    } else if lower.starts_with("mac os") || lower.starts_with("macos") {
        "macOS"
    } else if lower.starts_with("android") {
        "Android"
    } else if lower.starts_with("chromeos") {
        "ChromeOS"
    } else if lower.starts_with("linux") {
        "Linux"
    } else {
        return clean(os);
    };
    Some(family.to_string())
}

/// Detect the OS family from raw UA text (for UAs woothee can't place at all).
/// iOS markers are checked before macOS ("like Mac OS X") and Android before
/// Linux (Android UAs contain "Linux").
fn sniff_os(agent: &str) -> Option<String> {
    let lower = agent.to_lowercase();
    let has = |needle: &str| contains_word(&lower, needle);

    let family = if has("iphone") || has("ipad") || has("ipod") || has("ios") {
        "iOS"
    } else if has("android") {
        "Android"
    } else if has("windows") {
        "Windows"
    } else if lower.contains("mac os x") || has("macos") || has("macintosh") || has("darwin") {
        "macOS"
    } else if has("cros") {
        "ChromeOS"
    } else if has("linux") || has("ubuntu") || has("debian") || has("fedora") {
        "Linux"
    } else {
        return None;
    };
    Some(family.to_string())
}

/// Substring match with alphanumeric word boundaries on both sides, so "ios"
/// matches in "iOS/17.1" but not in "bios".
fn contains_word(haystack: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(at) = haystack[from..].find(needle) {
        let start = from + at;
        let end = start + needle.len();
        let bounded_left = start == 0 || !haystack.as_bytes()[start - 1].is_ascii_alphanumeric();
        let bounded_right =
            end == haystack.len() || !haystack.as_bytes()[end].is_ascii_alphanumeric();
        if bounded_left && bounded_right {
            return true;
        }
        from = start + 1;
    }
    false
}

/// The first product token of an application UA: `Name[/Version]`, skipping
/// parenthesized platform comments, URLs, bare version numbers, and the
/// structural tokens browsers emit for compatibility ("Mozilla/5.0", ...).
/// The version comes from the matched token when it carries one, otherwise
/// from the first version-shaped number outside any parenthesized section.
fn product_token(agent: &str) -> (Option<String>, Option<String>) {
    const STRUCTURAL: &[&str] = &[
        "mozilla",
        "compatible",
        "applewebkit",
        "khtml",
        "gecko",
        "like",
    ];

    let mut depth = 0usize;
    for token in agent.split_whitespace() {
        // Skip parenthesized comment sections: platform details, not products.
        let opens = token.matches('(').count();
        let closes = token.matches(')').count();
        if depth > 0 || opens > 0 {
            depth = (depth + opens).saturating_sub(closes);
            continue;
        }
        if token.contains("://") {
            continue;
        }

        let (name, token_version) = match token.split_once('/') {
            Some((name, version)) => (name, Some(version)),
            None => (token, None),
        };
        let name = name.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if name.is_empty()
            || looks_like_version(name)
            || STRUCTURAL.contains(&name.to_ascii_lowercase().as_str())
        {
            continue;
        }

        let version = token_version
            .and_then(find_version)
            .or_else(|| find_version(&strip_parens(agent)));
        return (Some(name.to_string()), version);
    }
    (None, None)
}

/// A token made only of digits and dots is a version, not a product name.
fn looks_like_version(token: &str) -> bool {
    !token.is_empty() && token.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// The first word-bounded `\d+(\.\d+)+` match in `s` (e.g. "1.2.3", but not
/// the "64" in "x64" nor a partial "2.3" from inside "v1.2.3"). A single `v`
/// prefix is tolerated, so "v1.2.3" yields "1.2.3".
fn find_version(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }

        // A digit run glued to a preceding word character or dot is part of a
        // larger token — consume and skip it.
        let bounded_left = i == 0 || {
            let prev = bytes[i - 1];
            let v_prefixed =
                (prev == b'v' || prev == b'V') && (i < 2 || !bytes[i - 2].is_ascii_alphanumeric());
            v_prefixed || (!prev.is_ascii_alphanumeric() && prev != b'.')
        };

        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let mut dotted_groups = 0;
        while i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            dotted_groups += 1;
        }
        let bounded_right = i == bytes.len() || !bytes[i].is_ascii_alphanumeric();

        if bounded_left && bounded_right && dotted_groups >= 1 {
            return Some(s[start..i].to_string());
        }
    }
    None
}

/// The UA text with every parenthesized section removed (version-number scans
/// must not pick OS versions out of platform details).
fn strip_parens(agent: &str) -> String {
    let mut out = String::with_capacity(agent.len());
    let mut depth = 0usize;
    for ch in agent.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_a_desktop_browser() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
        let c = classify(ua);
        assert_eq!(c.kind, UaKind::Desktop);
        assert_eq!(c.app.as_deref(), Some("Chrome"));
        assert_eq!(c.version.as_deref(), Some("120.0.0.0"));
        assert_eq!(c.os.as_deref(), Some("Windows"));
    }

    #[test]
    fn classifies_a_mobile_browser() {
        let ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_1 like Mac OS X) \
                  AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 \
                  Mobile/15E148 Safari/604.1";
        let c = classify(ua);
        assert_eq!(c.kind, UaKind::Mobile);
        assert_eq!(c.app.as_deref(), Some("Safari"));
        assert_eq!(c.os.as_deref(), Some("iOS"));
    }

    #[test]
    fn classifies_a_bare_application() {
        let c = classify("curl/8.5.0");
        assert_eq!(c.kind, UaKind::App);
        assert_eq!(c.app.as_deref(), Some("curl"));
        assert_eq!(c.version.as_deref(), Some("8.5.0"));
        assert_eq!(c.os, None);
    }

    #[test]
    fn classifies_an_application_with_platform_details() {
        let c = classify("MyApp/2.4.1 (Windows NT 10.0; Win64; x64)");
        assert_eq!(c.kind, UaKind::App);
        assert_eq!(c.app.as_deref(), Some("MyApp"));
        assert_eq!(c.version.as_deref(), Some("2.4.1"));
        assert_eq!(c.os.as_deref(), Some("Windows"));
    }

    #[test]
    fn application_version_is_not_taken_from_platform_details() {
        // No version on the product token; the "10.0" inside the parens must
        // not be mistaken for one, while a bare trailing version is accepted.
        let c = classify("MyApp (Windows NT 10.0)");
        assert_eq!(c.app.as_deref(), Some("MyApp"));
        assert_eq!(c.version, None);

        let c = classify("MyApp (Windows NT 10.0) 2.4");
        assert_eq!(c.version.as_deref(), Some("2.4"));
    }

    #[test]
    fn v_prefixed_versions_are_recognized() {
        let c = classify("MyApp/v1.2.3");
        assert_eq!(c.version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn ios_app_via_cfnetwork_gets_the_os() {
        let c = classify("MyApp/1.2 CFNetwork/1494.0.7 Darwin/23.4.0");
        assert_eq!(c.kind, UaKind::App);
        assert_eq!(c.app.as_deref(), Some("MyApp"));
        assert_eq!(c.version.as_deref(), Some("1.2"));
        assert_eq!(c.os.as_deref(), Some("iOS"));
    }

    #[test]
    fn sniffs_the_os_of_unknown_agents() {
        assert_eq!(
            classify("MyApp/1.0 (Android 14; SM-G991B)").os.as_deref(),
            Some("Android")
        );
        assert_eq!(
            classify("MyApp/1.0 (X11; Ubuntu)").os.as_deref(),
            Some("Linux")
        );
        assert_eq!(
            classify("MyApp/1.0 Darwin/23.0").os.as_deref(),
            Some("macOS")
        );
        // "bios" must not read as iOS.
        assert_eq!(classify("smartbios-updater/1.0").os, None);
    }

    #[test]
    fn apps_named_like_bots_are_bots() {
        assert_eq!(classify("TelegramBot (like TwitterBot)").kind, UaKind::Bot);
        assert_eq!(classify("my-crawler/1.0").kind, UaKind::Bot);
        assert_eq!(
            classify("FooSpider/2.1 (+https://foo.example)").kind,
            UaKind::Bot
        );
    }

    #[test]
    fn flags_known_crawlers_and_empty_agents() {
        assert_eq!(classify("").kind, UaKind::Bot);
        assert_eq!(classify("   ").kind, UaKind::Bot);
        assert_eq!(
            classify("Googlebot/2.1 (+http://www.google.com/bot.html)").kind,
            UaKind::Bot
        );
    }

    #[test]
    fn unrecognizable_noise_is_a_bot() {
        assert_eq!(classify("()").kind, UaKind::Bot);
        assert_eq!(classify("1.2.3").kind, UaKind::Bot);
    }

    #[test]
    fn structural_browser_tokens_are_not_products() {
        // An unknown Mozilla-prefixed client yields its real product token.
        let c = classify("Mozilla/5.0 (Windows NT 10.0; Win64; x64) NicheBrowser/3.1");
        assert_eq!(c.app.as_deref(), Some("NicheBrowser"));
        assert_eq!(c.version.as_deref(), Some("3.1"));
        assert_eq!(c.os.as_deref(), Some("Windows"));

        // Platform-only UAs keep the OS but claim no app.
        let c = classify("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
        assert_eq!(c.app, None);
        assert_eq!(c.os.as_deref(), Some("Windows"));
        assert_eq!(c.kind, UaKind::App);
    }

    #[test]
    fn find_version_requires_word_boundaries_and_a_dot() {
        assert_eq!(find_version("8.5.0"), Some("8.5.0".to_string()));
        assert_eq!(find_version("x64 build 1.2"), Some("1.2".to_string()));
        assert_eq!(find_version("v1.2.3"), Some("1.2.3".to_string()));
        assert_eq!(find_version("HTTP/2"), None); // no dotted group
        assert_eq!(find_version("sha256.1a2b"), None); // glued to a token
        assert_eq!(find_version("1.2beta"), None); // no right boundary
    }
}
