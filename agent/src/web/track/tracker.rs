//! `GET /tracker.js` — the embedded JavaScript beacon.

use actix_web::http::header::CACHE_CONTROL;
use actix_web::HttpResponse;

const TRACKER_JS: &str = include_str!("tracker.js");

pub async fn tracker_js() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("application/javascript; charset=utf-8")
        .insert_header((CACHE_CONTROL, "public, max-age=3600"))
        .body(TRACKER_JS)
}
