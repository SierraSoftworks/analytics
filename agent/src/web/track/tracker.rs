//! `GET /tracker.js` — the embedded JavaScript beacon.
//!
//! The beacon lives in the `tracker/` project (pure JS, built into a single minified
//! artifact with esbuild). `agent/build.rs` guarantees `tracker/dist/tracker.js`
//! exists (with a placeholder) even when the beacon has not been built, so the agent
//! always compiles.

use actix_web::http::header::CACHE_CONTROL;
use actix_web::HttpResponse;

const TRACKER_JS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../tracker/dist/tracker.js"));

pub async fn tracker_js() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("application/javascript; charset=utf-8")
        .insert_header((CACHE_CONTROL, "public, max-age=3600"))
        .body(TRACKER_JS)
}
