//! Authenticated instance/runtime information for the Settings page.

use actix_web::{HttpResponse, web};
use analytics_api::Instance;

use crate::state::AppState;

/// `GET /api/v1/instance` — the running version and operational posture. It sits
/// behind `api_auth`, so (unlike the public `/health` endpoint) it may reveal the
/// version and configuration to a signed-in administrator.
pub async fn instance(state: web::Data<AppState>) -> HttpResponse {
    let cfg = &state.config;
    HttpResponse::Ok().json(Instance {
        version: crate::version!().to_string(),
        retention_days: cfg.storage.retention.as_secs() / 86_400,
        hot_window_hours: cfg.storage.hot_window.as_secs() / 3_600,
        honor_dnt: cfg.privacy.honor_dnt,
        rate_limiting: cfg.ratelimit.enabled,
        tracking_per_minute: cfg.ratelimit.tracking.per_minute,
        unauthenticated_per_minute: cfg.ratelimit.unauthenticated.per_minute,
        max_auto_sources: cfg.storage.max_auto_sources as u64,
    })
}
