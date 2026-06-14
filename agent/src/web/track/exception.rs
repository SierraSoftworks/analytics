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
    // Rate limiting + body size cap are applied by the /track scope middleware.
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
