//! `GET /robots.txt` — crawler directives.
//!
//! This service is an analytics backend and private dashboard; there is nothing
//! useful for search engines to index, so we ask well-behaved crawlers to stay out.

use actix_web::HttpResponse;
use actix_web::http::header::CACHE_CONTROL;

const ROBOTS_TXT: &str = "User-agent: *\nDisallow: /\n";

pub async fn robots_txt() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/plain; charset=utf-8")
        .insert_header((CACHE_CONTROL, "public, max-age=86400"))
        .body(ROBOTS_TXT)
}
