//! Custom/pixel event detail: one named event's aggregate, distributions,
//! metadata exemplars, and session traces.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, web};
use analytics_api::EventDetail;
use serde::Deserialize;
use tracing_batteries::prelude::*;

use super::query::resolve_range;
use super::{internal_error, json_error};
use crate::analytics::{self, filter::FieldSet};
use crate::state::AppState;

const VARIANT_LIMIT: usize = 50;

/// Query parameters for the event detail. The name rides in the query string
/// (not the path) because event names are reporter-chosen free text — slashes
/// and spaces must not break routing. `q` uses the dashboard field vocabulary,
/// so the detail covers the same slice as the panel that linked here.
#[derive(Deserialize)]
pub struct EventDetailQuery {
    pub name: String,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub q: Option<String>,
}

/// `GET /api/v1/events?name=…` — one event name in forensic detail.
///
/// Without an explicit range this looks across **all time**, anchored at the
/// earliest stored event so the trend buckets cover the data: an event linked
/// from an old bookmark must always open.
pub async fn detail(
    state: web::Data<AppState>,
    query: web::Query<EventDetailQuery>,
) -> HttpResponse {
    let query = query.into_inner();
    let store = state.store.clone();
    let parquet_dir = state.config.storage.parquet_dir.clone();

    let filter = match query.q.as_deref() {
        Some(q) => match analytics::filter::compile_query(q, FieldSet::Dashboard, &store) {
            Ok(filter) => filter,
            Err(message) => return json_error(StatusCode::BAD_REQUEST, message),
        },
        None => None,
    };

    let result = web::block(move || -> crate::errors::Result<Option<EventDetail>> {
        let from = match query.from {
            Some(f) if f <= 0 => analytics::earliest_event_ms(&store, &parquet_dir)?,
            other => other,
        };
        let (from, to, _) = resolve_range(from, query.to, None);
        analytics::event_detail(
            &store,
            &parquet_dir,
            &query.name,
            from,
            to,
            filter.as_ref(),
            VARIANT_LIMIT,
        )
    })
    .await;

    match result {
        Ok(Ok(Some(detail))) => HttpResponse::Ok().json(detail),
        Ok(Ok(None)) => json_error(StatusCode::NOT_FOUND, "Event not found."),
        Ok(Err(err)) => internal_error(err),
        Err(err) => {
            error!("event detail task failed: {err}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to load the event.",
            )
        }
    }
}
