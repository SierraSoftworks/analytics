//! Session traces: the full timeline of one visit's page views, custom events,
//! and exceptions, keyed by the tracker's per-visit session id.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, web};
use analytics_api::SessionTrace;
use chrono::Utc;
use serde::Deserialize;
use tracing_batteries::prelude::*;

use super::{internal_error, json_error};
use crate::analytics;
use crate::state::AppState;

/// The most events a returned timeline may carry (a runaway client reusing one
/// session id must not balloon the response).
const TRACE_EVENT_LIMIT: usize = 1_000;

#[derive(Deserialize)]
pub struct TraceQuery {
    from: Option<i64>,
    to: Option<i64>,
}

/// `GET /api/v1/traces/{session_id}` — one session's ordered event timeline.
///
/// Without an explicit range this looks across **all time**: a trace linked
/// from the dashboard's recent-traces sample, an exception exemplar, or an old
/// bookmark must always open.
pub async fn detail(
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<TraceQuery>,
) -> HttpResponse {
    let session_id = path.into_inner();
    let now = Utc::now().timestamp_millis();
    let to = query
        .to
        .unwrap_or(now)
        .clamp(1, super::query::MAX_INSTANT_MS);
    let from = query.from.unwrap_or(0).clamp(0, to - 1);
    let store = state.store.clone();
    let parquet_dir = state.config.storage.parquet_dir.clone();

    let result = web::block(move || -> crate::errors::Result<Option<SessionTrace>> {
        analytics::session_trace(
            &store,
            &parquet_dir,
            &session_id,
            from,
            to,
            TRACE_EVENT_LIMIT,
        )
    })
    .await;

    match result {
        Ok(Ok(Some(trace))) => HttpResponse::Ok().json(trace),
        Ok(Ok(None)) => json_error(StatusCode::NOT_FOUND, "Session trace not found."),
        Ok(Err(err)) => internal_error(err),
        Err(err) => {
            error!("session trace task failed: {err}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to load the session trace.",
            )
        }
    }
}
