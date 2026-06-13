//! `GET /track/ping` — daily unique-visitor oracle via the HTTP conditional-request
//! cache trick (no cookies, no IP storage).
//!
//! The browser caches the response per ping URL (the tracker includes the hostname),
//! revalidating on every page load because of `Cache-Control: no-cache`. The server
//! always returns 200 with a freshly-computed body and resets `Last-Modified` to
//! today's UTC midnight, so the conditional `If-Modified-Since` reveals whether the
//! browser has already pinged today.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use actix_web::http::header::{CACHE_CONTROL, IF_MODIFIED_SINCE, LAST_MODIFIED};
use actix_web::{HttpRequest, HttpResponse, web};
use analytics_api::PingResponse;
use chrono::{Datelike, TimeZone, Utc};

use crate::state::AppState;
use crate::web::extract;

pub async fn ping(req: HttpRequest, state: web::Data<AppState>) -> HttpResponse {
    if state.config.ratelimit.enabled {
        let ip = extract::client_ip(&req, state.config.web.trust_proxy);
        if !state.tracking_limiter.check(&ip) {
            return HttpResponse::TooManyRequests().finish();
        }
    }

    let today_midnight = today_midnight_ms();
    let last_seen = req
        .headers()
        .get(IF_MODIFIED_SINCE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| httpdate::parse_http_date(value).ok())
        .map(system_time_to_ms);
    let unique = match last_seen {
        Some(ms) => ms < today_midnight,
        None => true,
    };

    HttpResponse::Ok()
        .insert_header((
            LAST_MODIFIED,
            httpdate::fmt_http_date(ms_to_system_time(today_midnight)),
        ))
        .insert_header((CACHE_CONTROL, "no-cache, private"))
        .json(PingResponse { unique })
}

fn today_midnight_ms() -> i64 {
    let now = Utc::now();
    Utc.with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .single()
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0)
}

fn system_time_to_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn ms_to_system_time(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64)
}
