//! `GET /track/gif/{gif_id}.gif` — a pre-generated tracking pixel. Unknown ids are
//! rejected (404), so there is no open pixel.

use actix_web::http::header::CACHE_CONTROL;
use actix_web::{HttpResponse, web};
use analytics_api::pixel_source;
use chrono::Utc;

use crate::state::AppState;
use crate::store::{EventKind, StoredEvent};

/// A 1x1 transparent GIF.
const BLANK_GIF: &[u8] = include_bytes!("../../../assets/blank.gif");

// Rate limiting is applied by the /track scope middleware in `super`.
pub async fn gif(state: web::Data<AppState>, path: web::Path<String>) -> HttpResponse {
    // Strip the trailing `.gif` the route captures as part of the id.
    let id = path.into_inner();
    let id = id.strip_suffix(".gif").unwrap_or(&id);

    let pixel = match state.store.get_pixel(id) {
        Ok(Some(pixel)) => pixel,
        _ => return HttpResponse::NotFound().finish(),
    };

    let received_ms = Utc::now().timestamp_millis();
    let metadata_json = (!pixel.metadata.is_empty())
        .then(|| serde_json::to_string(&pixel.metadata).ok())
        .flatten();

    state.ingest.submit(StoredEvent {
        created_ms: received_ms,
        received_ms,
        kind: EventKind::Pixel,
        source: pixel_source(&pixel.id),
        event_name: Some(pixel.event_name),
        metadata_json,
        ..Default::default()
    });

    HttpResponse::Ok()
        .content_type("image/gif")
        .insert_header((
            CACHE_CONTROL,
            "no-store, no-cache, must-revalidate, private",
        ))
        .body(BLANK_GIF)
}
