use std::sync::Arc;

use crate::config::Config;
use crate::ingest::Ingest;
use crate::ratelimit::RateLimiter;
use crate::store::Store;

/// Shared application state, wrapped in `web::Data` (an `Arc`) and cloned per worker.
// `store` is consumed by the protected API (Phase 6) and `unauth_limiter` by the
// auth middleware (Phase 5); allow the forward-looking fields until then.
#[allow(dead_code)]
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub ingest: Ingest,
    pub config: Arc<Config>,
    /// Per-IP limiter for the public tracking endpoints.
    pub tracking_limiter: Arc<RateLimiter>,
    /// Per-IP limiter for unauthenticated hits to protected endpoints (Phase 5).
    pub unauth_limiter: Arc<RateLimiter>,
}
