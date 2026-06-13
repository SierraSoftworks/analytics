use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What kind of thing a source is. Both flow through the same event pipeline; the
/// distinction drives which metrics make sense in the dashboard. The kind defaults
/// from the source URI scheme but can be overridden by an administrator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    #[default]
    Website,
    Application,
}

/// The scheme of a source URI. Sources are identified by a URI so the model can be
/// extended with new kinds in future without changing the storage schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceScheme {
    /// `https://<hostname>` — a browser-tracked website.
    Website,
    /// `app://<appname>` — an application.
    Application,
    /// `pixel://<id>` — a pre-generated tracking GIF.
    Pixel,
    /// Anything else (forward compatibility).
    Other,
}

/// A source that reports events, identified by its canonical URI. Non-pixel sources
/// are created automatically on first sight (unassigned) and grouped into a project
/// via the admin UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// Canonical source URI (also the storage key), e.g. `https://example.com`.
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub kind: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<DateTime<Utc>>,
}

/// Payload for assigning/updating a source's project, kind, and display name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SourceInput {
    /// Send an empty string to unassign from any project.
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub kind: Option<SourceKind>,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// Canonical website source URI for a hostname.
pub fn website_source(hostname: &str) -> String {
    format!("https://{}", hostname.trim().trim_end_matches('.').to_lowercase())
}

/// Source URI for an application.
pub fn app_source(name: &str) -> String {
    format!("app://{}", name.trim())
}

/// Source URI for a tracking pixel id.
pub fn pixel_source(id: &str) -> String {
    format!("pixel://{id}")
}

/// Classify a source URI by its scheme.
pub fn source_scheme(source: &str) -> SourceScheme {
    if source.starts_with("https://") || source.starts_with("http://") {
        SourceScheme::Website
    } else if source.starts_with("app://") {
        SourceScheme::Application
    } else if source.starts_with("pixel://") {
        SourceScheme::Pixel
    } else {
        SourceScheme::Other
    }
}

/// Extract the pixel id from a `pixel://<id>` URI, if applicable.
pub fn pixel_id_of(source: &str) -> Option<&str> {
    source.strip_prefix("pixel://")
}

/// A human-friendly label for a source URI (scheme stripped).
pub fn source_label(source: &str) -> &str {
    source
        .strip_prefix("https://")
        .or_else(|| source.strip_prefix("http://"))
        .or_else(|| source.strip_prefix("app://"))
        .or_else(|| source.strip_prefix("pixel://"))
        .unwrap_or(source)
}

/// The source kind implied by a URI scheme, used when auto-registering a source.
pub fn default_kind(source: &str) -> SourceKind {
    match source_scheme(source) {
        SourceScheme::Application => SourceKind::Application,
        _ => SourceKind::Website,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_canonical_uris() {
        assert_eq!(website_source("Example.COM"), "https://example.com");
        assert_eq!(website_source("example.com."), "https://example.com");
        assert_eq!(app_source("myapp"), "app://myapp");
        assert_eq!(pixel_source("01HX"), "pixel://01HX");
    }

    #[test]
    fn classifies_and_extracts() {
        assert_eq!(source_scheme("https://a.com"), SourceScheme::Website);
        assert_eq!(source_scheme("app://a"), SourceScheme::Application);
        assert_eq!(source_scheme("pixel://x"), SourceScheme::Pixel);
        assert_eq!(source_scheme("ftp://x"), SourceScheme::Other);

        assert_eq!(pixel_id_of("pixel://abc"), Some("abc"));
        assert_eq!(pixel_id_of("https://a.com"), None);

        assert_eq!(source_label("https://a.com/x"), "a.com/x");
        assert_eq!(source_label("pixel://abc"), "abc");

        assert_eq!(default_kind("app://a"), SourceKind::Application);
        assert_eq!(default_kind("https://a.com"), SourceKind::Website);
    }
}
