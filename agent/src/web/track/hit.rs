//! `POST /track/hit` — record a page view / unload / custom event.

use actix_web::{HttpRequest, HttpResponse, web};
use analytics_api::TrackEvent;
use chrono::Utc;

use crate::ingest;
use crate::state::AppState;
use crate::web::extract;

pub async fn hit(
    req: HttpRequest,
    state: web::Data<AppState>,
    payload: web::Json<TrackEvent>,
) -> HttpResponse {
    if state.config.ratelimit.enabled {
        let ip = extract::client_ip(&req, state.config.web.trust_proxy);
        if !state.tracking_limiter.check(&ip) {
            return HttpResponse::TooManyRequests().finish();
        }
    }

    // Respect Do-Not-Track / GPC (the tracker also checks client-side).
    if state.config.privacy.honor_dnt && extract::privacy_signal(&req) {
        return HttpResponse::NoContent().finish();
    }

    let user_agent = extract::header(&req, "user-agent").unwrap_or_default();
    let accept_language = extract::header(&req, "accept-language");
    let received_ms = Utc::now().timestamp_millis();

    // Bots / unparseable URLs yield `None`; respond 204 either way so the outcome is
    // not observable to the caller.
    if let Some(event) = ingest::build_event(
        payload.into_inner(),
        &user_agent,
        accept_language.as_deref(),
        received_ms,
    ) {
        state.ingest.submit(event);
    }

    HttpResponse::NoContent().finish()
}
