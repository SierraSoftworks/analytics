//! User-agent classification. The raw UA string is parsed into broad classes
//! (browser, OS, device) and a bot flag; the raw string is never stored.

use woothee::parser::Parser;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct UaClasses {
    pub browser: Option<String>,
    pub os: Option<String>,
    pub device: Option<String>,
    pub is_bot: bool,
}

/// Classify a User-Agent header value into non-identifying classes.
pub fn classify(user_agent: &str) -> UaClasses {
    if user_agent.trim().is_empty() {
        return UaClasses {
            is_bot: true,
            ..Default::default()
        };
    }

    match Parser::new().parse(user_agent) {
        Some(result) => {
            let is_bot = result.category.eq_ignore_ascii_case("crawler")
                || result.name.eq_ignore_ascii_case("crawler");
            UaClasses {
                browser: clean(result.name),
                os: clean(result.os),
                device: clean(result.category),
                is_bot,
            }
        }
        None => UaClasses {
            is_bot: true,
            ..Default::default()
        },
    }
}

fn clean(value: &str) -> Option<String> {
    if value.is_empty() || value.eq_ignore_ascii_case("UNKNOWN") {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_a_desktop_browser() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
        let c = classify(ua);
        assert!(!c.is_bot);
        assert_eq!(c.browser.as_deref(), Some("Chrome"));
        assert!(c.os.is_some());
        assert!(c.device.is_some());
    }

    #[test]
    fn flags_bots_and_empty() {
        assert!(classify("").is_bot);
        assert!(classify("Googlebot/2.1 (+http://www.google.com/bot.html)").is_bot);
    }
}
