//! The global overview across all projects.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, web};
use tracing_batteries::prelude::*;

use super::query::{StatsQuery, resolve_range};
use super::{internal_error, json_error};
use crate::analytics;
use crate::state::AppState;

pub async fn overview(state: web::Data<AppState>, query: web::Query<StatsQuery>) -> HttpResponse {
    let (from, to, bucket) = resolve_range(&query);
    let store = state.store.clone();
    let parquet_dir = state.config.storage.parquet_dir.clone();

    let result =
        web::block(move || analytics::overview(&store, &parquet_dir, from, to, bucket)).await;

    match result {
        Ok(Ok(overview)) => HttpResponse::Ok().json(overview),
        Ok(Err(err)) => internal_error(err),
        Err(err) => {
            error!("overview computation task failed: {err}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to compute the overview.",
            )
        }
    }
}
