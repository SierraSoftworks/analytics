use std::sync::Arc;

use crate::config::Config;
use crate::ingest::Ingest;
use crate::ratelimit::RateLimiter;
use crate::store::Store;
use crate::web::helpers::oidc::OidcCache;

/// Shared application state, wrapped in `web::Data` (an `Arc`) and cloned per worker.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub ingest: Ingest,
    pub config: Arc<Config>,
    /// HTTP client for OIDC discovery/JWKS/token exchange.
    pub http: reqwest::Client,
    /// Cached OIDC discovery document + signing keys.
    pub oidc_cache: Arc<OidcCache>,
    /// The parsed admin ACL (filt-rs is not `Clone`, so it lives behind an `Arc`).
    pub acl: Arc<filt_rs::Filter>,
    /// Per-IP limiter for the public tracking endpoints.
    pub tracking_limiter: Arc<RateLimiter>,
    /// Per-IP limiter for unauthenticated hits to protected endpoints.
    pub unauth_limiter: Arc<RateLimiter>,
}
