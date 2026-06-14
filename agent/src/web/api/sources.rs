//! Source management: list every source (incl. unassigned) and assign/update or
//! delete one. Sources are identified by their URI via a query parameter to avoid
//! encoding a URI (with its `://`) in the path.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, web};
use analytics_api::SourceInput;
use serde::Deserialize;

use super::{internal_error, json_error};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct SourceRef {
    uri: String,
}

pub async fn list(state: web::Data<AppState>) -> HttpResponse {
    match state.store.list_sources() {
        Ok(mut sources) => {
            sources.sort_by(|a, b| a.uri.cmp(&b.uri));
            HttpResponse::Ok().json(sources)
        }
        Err(err) => internal_error(err),
    }
}

pub async fn update(
    state: web::Data<AppState>,
    query: web::Query<SourceRef>,
    body: web::Json<SourceInput>,
) -> HttpResponse {
    let input = body.into_inner();
    // Read-modify-write in a single transaction so concurrent edits can't clobber.
    let result = state.store.mutate_source(&query.uri, move |source| {
        if let Some(project_id) = input.project_id {
            // An empty string unassigns the source from any project.
            source.project_id = Some(project_id).filter(|p| !p.trim().is_empty());
        }
        if let Some(kind) = input.kind {
            source.kind = kind;
        }
        if let Some(display_name) = input.display_name {
            source.display_name = Some(display_name).filter(|n| !n.trim().is_empty());
        }
    });

    match result {
        Ok(Some(source)) => HttpResponse::Ok().json(source),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "Source not found."),
        Err(err) => internal_error(err),
    }
}

pub async fn delete(state: web::Data<AppState>, query: web::Query<SourceRef>) -> HttpResponse {
    match state.store.delete_source(&query.uri) {
        Ok(true) => HttpResponse::NoContent().finish(),
        Ok(false) => json_error(StatusCode::NOT_FOUND, "Source not found."),
        Err(err) => internal_error(err),
    }
}
