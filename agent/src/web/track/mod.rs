//! Public, unauthenticated tracking endpoints.

mod exception;
mod gif;
mod hit;
mod ping;
mod tracker;

use actix_cors::Cors;
use actix_web::body::BoxBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::middleware::{Next, from_fn};
use actix_web::{HttpResponse, web};

use crate::state::AppState;

/// A legitimate beacon/exception payload is small; cap the body well below actix's
/// 2 MB default to limit the work an unauthenticated flood can force.
const MAX_TRACK_BODY: usize = 16 * 1024;

/// Register `/tracker.js` and the CORS-enabled `/track/*` endpoints.
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/tracker.js", web::get().to(tracker::tracker_js)).service(
        // Beacons are sent cross-origin from tracked sites. Hits and exceptions are
        // posted as `text/plain` so they are CORS "simple requests" (no preflight) and
        // can be sent with `mode: "no-cors"`; `beacon_json_config` makes the JSON
        // extractor accept that content type. The `/ping` GET still needs CORS to
        // expose its JSON response to the page, so the scope stays permissively
        // CORS-enabled. No credentials are involved. Rate limiting runs as middleware
        // so over-limit requests are rejected before their bodies are read/parsed.
        web::scope("/track")
            .app_data(beacon_json_config())
            .wrap(from_fn(rate_limit))
            .wrap(Cors::permissive())
            .route("/ping", web::get().to(ping::ping))
            .route("/hit", web::post().to(hit::hit))
            .route("/exception", web::post().to(exception::exception))
            .route("/gif/{id}", web::get().to(gif::gif)),
    );
}

/// JSON body configuration for the beacon endpoints. Caps the body well below actix's
/// 2 MB default (a legitimate beacon is small) and accepts both `application/json`
/// (direct API callers) and `text/plain` — the latter lets browsers post beacons
/// preflight-free via `navigator.sendBeacon` or `fetch` with `mode: "no-cors"`.
fn beacon_json_config() -> web::JsonConfig {
    web::JsonConfig::default()
        .limit(MAX_TRACK_BODY)
        .content_type(|mime| {
            let essence = mime.essence_str();
            essence == "application/json" || essence == "text/plain"
        })
}

/// Per-IP token-bucket limit for the public tracking endpoints, applied before the
/// handler runs. The IP is a transient limiter key only — never stored or logged.
async fn rate_limit(
    req: ServiceRequest,
    next: Next<BoxBody>,
) -> Result<ServiceResponse<BoxBody>, actix_web::Error> {
    if let Some(state) = req.app_data::<web::Data<AppState>>().cloned()
        && state.config.ratelimit.enabled
    {
        let ip = crate::web::extract::client_ip(req.request(), state.config.web.trust_proxy);
        if !state.tracking_limiter.check(&ip) {
            return Ok(req.into_response(HttpResponse::TooManyRequests().finish()));
        }
    }
    next.call(req).await
}
