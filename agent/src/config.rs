use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

/// Top-level server configuration, loaded from a YAML file.
///
/// All fields have sensible defaults so that a config file is optional for local
/// development. Secrets can be injected with `${{ env.VAR_NAME }}` placeholders,
/// which are interpolated from the process environment before parsing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub web: WebConfig,
    pub storage: StorageConfig,
    pub privacy: PrivacyConfig,
    pub telemetry: TelemetryConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    /// The `host:port` the server listens on.
    pub address: String,
    /// The externally visible base URL, used to build absolute redirect URIs.
    pub base_url: Option<String>,
    /// Trust `X-Forwarded-*` headers from an upstream reverse proxy.
    pub trust_proxy: bool,
    pub admin: AdminConfig,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            address: "127.0.0.1:8080".to_string(),
            base_url: None,
            trust_proxy: false,
            admin: AdminConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AdminConfig {
    /// An ACL filter expression evaluated for every protected request. Defaults to
    /// `false` (deny all) so the dashboard is locked down unless explicitly opened.
    pub acl: String,
    /// OIDC provider configuration. When absent, authentication is disabled and the
    /// ACL is evaluated against request metadata only.
    pub oidc: Option<OidcConfig>,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            acl: "false".to_string(),
            oidc: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct OidcConfig {
    /// The issuer/discovery endpoint (without the `.well-known` suffix).
    pub endpoint: String,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Path to the redb file used as the append-only hot store.
    pub redb_path: String,
    /// Directory holding the rolled-up Parquet partitions (the cold archive).
    pub parquet_dir: String,
    /// How long events stay in redb before being compacted to Parquet.
    #[serde(with = "humantime_serde")]
    pub hot_window: Duration,
    /// How often the compactor seals redb windows into Parquet.
    #[serde(with = "humantime_serde")]
    pub rollup_interval: Duration,
    /// How long Parquet partitions are retained before deletion.
    #[serde(with = "humantime_serde")]
    pub retention: Duration,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            redb_path: "analytics.redb".to_string(),
            parquet_dir: "parquet-store".to_string(),
            hot_window: Duration::from_secs(48 * 60 * 60),
            rollup_interval: Duration::from_secs(60 * 60),
            retention: Duration::from_secs(365 * 24 * 60 * 60),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    /// How often the hashing salt is rotated (uniqueness is primarily cache-header
    /// based; this salt only guards any auxiliary hashing).
    #[serde(with = "humantime_serde")]
    pub salt_rotation: Duration,
    /// Honour the `DNT`/`Sec-GPC` request signals by dropping the beacon.
    pub honor_dnt: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            salt_rotation: Duration::from_secs(24 * 60 * 60),
            honor_dnt: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    pub service_name: String,
    pub sentry_dsn: Option<String>,
    pub environment: Option<String>,
    pub otlp_endpoint: Option<String>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            service_name: "analytics".to_string(),
            sentry_dsn: None,
            environment: None,
            otlp_endpoint: None,
        }
    }
}

impl Config {
    /// Load configuration from `path`. A missing file yields the default config so
    /// that `cargo run` works out of the box.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Config::default());
        }

        let raw = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::Io(path.display().to_string(), e))?;
        Self::from_yaml_str(&raw)
    }

    /// Parse a YAML document, interpolating `${{ env.VAR }}` placeholders inside
    /// string *values* (so placeholders in comments are ignored).
    pub fn from_yaml_str(raw: &str) -> Result<Self, ConfigError> {
        if raw.trim().is_empty() {
            return Ok(Config::default());
        }

        let mut value: serde_yaml::Value = serde_yaml::from_str(raw).map_err(ConfigError::Parse)?;
        interpolate_value(&mut value)?;
        serde_yaml::from_value(value).map_err(ConfigError::Parse)
    }
}

/// Recursively interpolate environment placeholders in every string value of a
/// parsed YAML document.
fn interpolate_value(value: &mut serde_yaml::Value) -> Result<(), ConfigError> {
    match value {
        serde_yaml::Value::String(s) => *s = interpolate(s)?,
        serde_yaml::Value::Sequence(seq) => {
            for item in seq {
                interpolate_value(item)?;
            }
        }
        serde_yaml::Value::Mapping(map) => {
            for (_key, val) in map.iter_mut() {
                interpolate_value(val)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Replace `${{ env.VAR_NAME }}` placeholders with values from the environment.
fn interpolate(input: &str) -> Result<String, ConfigError> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let end = after
            .find("}}")
            .ok_or_else(|| ConfigError::Interpolation("unterminated `${{ ... }}` placeholder".into()))?;
        let expr = after[..end].trim();
        let var = expr
            .strip_prefix("env.")
            .ok_or_else(|| ConfigError::Interpolation(format!("unsupported placeholder expression: `{expr}`")))?
            .trim();
        let value = std::env::var(var)
            .map_err(|_| ConfigError::Interpolation(format!("environment variable not set: `{var}`")))?;
        out.push_str(&value);
        rest = &after[end + 2..];
    }

    out.push_str(rest);
    Ok(out)
}

#[derive(Debug)]
pub enum ConfigError {
    Io(String, std::io::Error),
    Parse(serde_yaml::Error),
    Interpolation(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(path, err) => write!(f, "failed to read config file `{path}`: {err}"),
            ConfigError::Parse(err) => write!(f, "failed to parse YAML config: {err}"),
            ConfigError::Interpolation(msg) => write!(f, "config interpolation error: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_for_empty_document() {
        for doc in ["", "{}", "   \n"] {
            let config = Config::from_yaml_str(doc).unwrap();
            assert_eq!(config.web.address, "127.0.0.1:8080");
            assert_eq!(config.storage.redb_path, "analytics.redb");
            assert!(config.privacy.honor_dnt);
            assert!(config.web.admin.oidc.is_none());
        }
    }

    #[test]
    fn interpolates_environment_placeholders_in_values() {
        // SAFETY: single-threaded test setting a process-local variable.
        unsafe { std::env::set_var("ANALYTICS_TEST_DSN", "https://example/dsn") };
        let config =
            Config::from_yaml_str("telemetry:\n  sentry_dsn: ${{ env.ANALYTICS_TEST_DSN }}\n")
                .unwrap();
        assert_eq!(config.telemetry.sentry_dsn.as_deref(), Some("https://example/dsn"));
    }

    #[test]
    fn placeholders_in_comments_are_ignored() {
        // The YAML parser drops comments, so an unset placeholder inside one must not
        // cause an interpolation error (regression: example config failed to load).
        let config =
            Config::from_yaml_str("# uses ${{ env.NOT_SET_ANALYTICS }}\nweb:\n  trust_proxy: true\n")
                .unwrap();
        assert!(config.web.trust_proxy);
    }

    #[test]
    fn missing_environment_variable_in_a_value_is_an_error() {
        let err =
            Config::from_yaml_str("telemetry:\n  sentry_dsn: ${{ env.DEFINITELY_NOT_SET_ANALYTICS }}\n")
                .unwrap_err();
        assert!(matches!(err, ConfigError::Interpolation(_)));
    }

    #[test]
    fn parses_humantime_durations() {
        let config =
            Config::from_yaml_str("storage:\n  hot_window: 12h\n  retention: 30d\n").unwrap();
        assert_eq!(config.storage.hot_window, Duration::from_secs(12 * 3600));
        assert_eq!(config.storage.retention, Duration::from_secs(30 * 24 * 3600));
    }

    #[test]
    fn example_config_loads() {
        let raw = include_str!("../../config.example.yaml");
        let config = Config::from_yaml_str(raw).expect("example config should load");
        assert_eq!(config.web.address, "127.0.0.1:8080");
    }
}
