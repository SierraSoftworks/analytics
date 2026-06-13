//! `POST /track/exception` — report a client-side exception.

use actix_web::{HttpRequest, HttpResponse, web};
use analytics_api::ExceptionReport;
use chrono::Utc;

use crate::ingest;
use crate::state::AppState;
use crate::web::extract;

pub async fn exception(
    req: HttpRequest,
    state: web::Data<AppState>,
    payload: web::Json<ExceptionReport>,
) -> HttpResponse {
    if state.config.ratelimit.enabled {
        let ip = extract::client_ip(&req, state.config.web.trust_proxy);
        if !state.tracking_limiter.check(&ip) {
            return HttpResponse::TooManyRequests().finish();
        }
    }

    if state.config.privacy.honor_dnt && extract::privacy_signal(&req) {
        return HttpResponse::NoContent().finish();
    }

    let user_agent = extract::header(&req, "user-agent").unwrap_or_default();
    let received_ms = Utc::now().timestamp_millis();

    if let Some(event) = ingest::build_exception(payload.into_inner(), &user_agent, received_ms) {
        state.ingest.submit(event);
    }

    HttpResponse::NoContent().finish()
}
