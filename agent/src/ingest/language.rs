//! Derive a coarse language from the `Accept-Language` header. The raw header is
//! never stored — only the primary base language tag (e.g. "en").

/// Extract the primary base language subtag from an `Accept-Language` header.
pub fn primary_language(accept_language: &str) -> Option<String> {
    let first = accept_language.split(',').next()?;
    let tag = first.split(';').next().unwrap_or("").trim();
    let base = tag.split('-').next().unwrap_or("").trim().to_lowercase();
    if base.is_empty() || !base.chars().all(|c| c.is_ascii_alphabetic()) {
        None
    } else {
        Some(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_primary_language() {
        assert_eq!(primary_language("en-US,en;q=0.9,fr;q=0.8").as_deref(), Some("en"));
        assert_eq!(primary_language("de").as_deref(), Some("de"));
        assert_eq!(primary_language("*").as_deref(), None);
        assert_eq!(primary_language("").as_deref(), None);
    }
}
