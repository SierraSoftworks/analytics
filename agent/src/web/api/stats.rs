//! The unified dashboard statistics endpoint. One filterable query serves the
//! global view and every drill-down of it — filtering is a single `q`
//! expression (filt-rs syntax) compiled to a polars predicate, so the UI can
//! compose arbitrarily complex filters without switching endpoints.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, web};
use analytics_api::DashboardQuery;
use tracing_batteries::prelude::*;

use super::query::resolve_range;
use super::{internal_error, json_error};
use crate::analytics::{self, filter::FieldSet};
use crate::state::AppState;

/// `GET /api/v1/stats` — the full dashboard payload for a time range and filter
/// expression. A malformed or unsupported `q` is the caller's error: 400 with a
/// message the UI can show under its query bar.
pub async fn stats(state: web::Data<AppState>, query: web::Query<DashboardQuery>) -> HttpResponse {
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

    let result = web::block(move || {
        // `from=0` means "all time": anchor the window at the earliest stored
        // event so the series isn't padded back to 1970 with empty buckets.
        let from = match query.from {
            Some(f) if f <= 0 => analytics::earliest_event_ms(&store, &parquet_dir)?,
            other => other,
        };
        let (from, to, bucket) = resolve_range(from, query.to, query.interval.as_deref());
        analytics::dashboard(&store, &parquet_dir, filter.as_ref(), from, to, bucket)
    })
    .await;

    match result {
        Ok(Ok(dashboard)) => HttpResponse::Ok().json(dashboard),
        Ok(Err(err)) => internal_error(err),
        Err(err) => {
            error!("dashboard computation task failed: {err}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to compute statistics.",
            )
        }
    }
}
