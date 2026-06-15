use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::errors::{Result, ResultExt};

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
    pub ratelimit: RateLimitConfig,
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
    /// A [filt-rs](https://github.com/SierraSoftworks/filters) ACL expression
    /// evaluated for every protected request. Defaults to `false` (deny all) so the
    /// dashboard is locked down unless explicitly opened.
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

impl AdminConfig {
    /// Parse the ACL expression into an evaluable filter, surfacing syntax errors
    /// with guidance.
    pub fn acl_filter(&self) -> Result<filt_rs::Filter> {
        filt_rs::Filter::new(self.acl.as_str()).wrap_user_err(
            "The `web.admin.acl` filter expression is invalid.",
            &[
                "Check the filter syntax: string literals use double quotes and membership uses the `in` operator.",
                "Example: \"administrators\" in claims.groups",
            ],
        )
    }
}

// Fields are consumed by the OIDC auth layer in Phase 5.
#[allow(dead_code)]
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
    /// Ceiling on auto-registered (unassigned) sources. Unknown reporting hostnames
    /// register automatically so nothing is dropped from the overview, but a flood of
    /// attacker-rotated hostnames must not grow the source table without bound. Once
    /// this many sources exist, new ones stop auto-registering (their events are still
    /// stored). Raise it if you legitimately track more distinct sources.
    pub max_auto_sources: usize,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            redb_path: "analytics.redb".to_string(),
            parquet_dir: "parquet-store".to_string(),
            hot_window: Duration::from_secs(48 * 60 * 60),
            rollup_interval: Duration::from_secs(60 * 60),
            retention: Duration::from_secs(365 * 24 * 60 * 60),
            max_auto_sources: 10_000,
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

/// Per-IP rate limiting. IPs are used only as transient in-memory keys and are
/// never logged or stored.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RateLimitConfig {
    pub enabled: bool,
    /// Limit applied per IP to the public tracking endpoints.
    pub tracking: RateLimitRule,
    /// Limit applied per IP to unauthenticated requests against protected endpoints.
    pub unauthenticated: RateLimitRule,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            tracking: RateLimitRule {
                per_minute: 600,
                burst: 200,
            },
            unauthenticated: RateLimitRule {
                per_minute: 60,
                burst: 20,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct RateLimitRule {
    /// Sustained requests per minute (token refill rate).
    pub per_minute: u32,
    /// Maximum burst (token bucket capacity).
    pub burst: u32,
}

impl Default for RateLimitRule {
    fn default() -> Self {
        Self {
            per_minute: 600,
            burst: 200,
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
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Config::default());
        }

        let raw = std::fs::read_to_string(path).wrap_user_err(
            format!("Could not read the configuration file `{}`.", path.display()),
            &["Check that the path is correct and that the file is readable."],
        )?;
        Self::from_yaml_str(&raw)
    }

    /// Parse a YAML document, interpolating `${{ env.VAR }}` placeholders inside
    /// string *values* (so placeholders in comments are ignored) and validating the
    /// ACL expression.
    pub fn from_yaml_str(raw: &str) -> Result<Self> {
        if raw.trim().is_empty() {
            return Ok(Config::default());
        }

        let mut value: serde_yaml::Value = serde_yaml::from_str(raw).wrap_user_err(
            "The configuration file is not valid YAML.",
            &["Check the file for syntax errors such as bad indentation or quoting."],
        )?;
        interpolate_value(&mut value)?;
        let config: Config = serde_yaml::from_value(value).wrap_user_err(
            "The configuration file does not match the expected schema.",
            &["Compare your configuration against config.example.yaml."],
        )?;

        // Fail fast on an invalid ACL rather than at the first request.
        config.web.admin.acl_filter()?;
        Ok(config)
    }
}

/// Recursively interpolate environment placeholders in every string value of a
/// parsed YAML document.
fn interpolate_value(value: &mut serde_yaml::Value) -> Result<()> {
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
fn interpolate(input: &str) -> Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let end = after.find("}}").ok_or_else(|| {
            human_errors::user(
                "A `${{ ... }}` placeholder in the configuration was not terminated.",
                &["Close every `${{` with a matching `}}`."],
            )
        })?;
        let expr = after[..end].trim();
        let var = expr
            .strip_prefix("env.")
            .ok_or_else(|| {
                human_errors::user(
                    format!("Unsupported configuration placeholder `{expr}`."),
                    &["Only `${{ env.VAR_NAME }}` placeholders are supported."],
                )
            })?
            .trim();
        let value = std::env::var(var).map_err(|_| {
            human_errors::user(
                format!("The environment variable `{var}` referenced in the configuration is not set."),
                &["Set the variable in the environment or in the .env file before starting the server."],
            )
        })?;
        out.push_str(&value);
        rest = &after[end + 2..];
    }

    out.push_str(rest);
    Ok(out)
}

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
            assert_eq!(config.web.admin.acl, "false");
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
        let config = Config::from_yaml_str(
            "# uses ${{ env.NOT_SET_ANALYTICS }}\nweb:\n  trust_proxy: true\n",
        )
        .unwrap();
        assert!(config.web.trust_proxy);
    }

    #[test]
    fn missing_environment_variable_in_a_value_is_an_error() {
        let err = Config::from_yaml_str(
            "telemetry:\n  sentry_dsn: ${{ env.DEFINITELY_NOT_SET_ANALYTICS }}\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("DEFINITELY_NOT_SET_ANALYTICS"));
    }

    #[test]
    fn parses_humantime_durations() {
        let config =
            Config::from_yaml_str("storage:\n  hot_window: 12h\n  retention: 30d\n").unwrap();
        assert_eq!(config.storage.hot_window, Duration::from_secs(12 * 3600));
        assert_eq!(config.storage.retention, Duration::from_secs(30 * 24 * 3600));
    }

    #[test]
    fn valid_acl_is_accepted() {
        let config =
            Config::from_yaml_str("web:\n  admin:\n    acl: '\"admins\" in claims.groups'\n")
                .unwrap();
        assert!(config.web.admin.acl_filter().is_ok());
    }

    #[test]
    fn invalid_acl_is_rejected() {
        let err = Config::from_yaml_str("web:\n  admin:\n    acl: '&& ||'\n").unwrap_err();
        assert!(err.to_string().contains("acl"));
    }

    #[test]
    fn example_config_loads() {
        let raw = include_str!("../../config.example.yaml");
        let config = Config::from_yaml_str(raw).expect("example config should load");
        // The example binds loopback; the exact port may be tuned in the sample file.
        assert!(config.web.address.starts_with("127.0.0.1:"));
    }
}
