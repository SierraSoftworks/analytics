//! Exception groups (Sentry-style): per-project listing, group detail, and triage.

use actix_web::http::StatusCode;
use actix_web::{HttpMessage, HttpRequest, HttpResponse, web};
use analytics_api::{ExceptionGroupDetail, TriageInput};
use chrono::Utc;
use serde::Deserialize;
use tracing_batteries::prelude::*;

use super::query::{StatsQuery, resolve_range};
use super::{Authenticated, internal_error, json_error};
use crate::analytics;
use crate::state::AppState;
use crate::store::ExceptionTriage;

const OCCURRENCE_LIMIT: u32 = 50;

/// `GET /api/v1/projects/{id}/exceptions` — grouped exceptions with triage status.
pub async fn list(
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<StatsQuery>,
) -> HttpResponse {
    let project_id = path.into_inner();
    let (from, to, _) = resolve_range(&query);
    let store = state.store.clone();
    let parquet_dir = state.config.storage.parquet_dir.clone();

    let result = web::block(move || -> crate::errors::Result<_> {
        let sources = analytics::project_source_uris(&store, &project_id)?;
        let mut groups = analytics::exception_groups(&store, &parquet_dir, &sources, from, to)?;
        for group in &mut groups {
            if let Some(triage) = store.get_triage(&project_id, &group.group_id)? {
                group.status = triage.status;
                group.note = triage.note;
            }
        }
        Ok(groups)
    })
    .await;

    match result {
        Ok(Ok(groups)) => HttpResponse::Ok().json(groups),
        Ok(Err(err)) => internal_error(err),
        Err(err) => {
            error!("exception listing task failed: {err}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to load exceptions.")
        }
    }
}

#[derive(Deserialize)]
pub struct DetailQuery {
    project: String,
    from: Option<i64>,
    to: Option<i64>,
}

/// `GET /api/v1/exceptions/{group_id}?project=…` — a group with recent occurrences.
pub async fn detail(
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<DetailQuery>,
) -> HttpResponse {
    let group_id = path.into_inner();
    let project_id = query.project.clone();
    let now = Utc::now().timestamp_millis();
    let to = query.to.unwrap_or(now);
    let from = query.from.unwrap_or(to - 30 * 86_400_000);
    let store = state.store.clone();
    let parquet_dir = state.config.storage.parquet_dir.clone();

    let result = web::block(move || -> crate::errors::Result<Option<ExceptionGroupDetail>> {
        let sources = analytics::project_source_uris(&store, &project_id)?;
        let mut group = match analytics::exception_groups(&store, &parquet_dir, &sources, from, to)?
            .into_iter()
            .find(|g| g.group_id == group_id)
        {
            Some(group) => group,
            None => return Ok(None),
        };
        if let Some(triage) = store.get_triage(&project_id, &group_id)? {
            group.status = triage.status;
            group.note = triage.note;
        }
        let occurrences = analytics::exception_occurrences(
            &store,
            &parquet_dir,
            &sources,
            &group_id,
            from,
            to,
            OCCURRENCE_LIMIT,
        )?;
        Ok(Some(ExceptionGroupDetail { group, occurrences }))
    })
    .await;

    match result {
        Ok(Ok(Some(detail))) => HttpResponse::Ok().json(detail),
        Ok(Ok(None)) => json_error(StatusCode::NOT_FOUND, "Exception group not found."),
        Ok(Err(err)) => internal_error(err),
        Err(err) => {
            error!("exception detail task failed: {err}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to load the exception.")
        }
    }
}

/// `PATCH /api/v1/exceptions/{group_id}` — set the triage status/note for a group.
pub async fn triage(
    req: HttpRequest,
    state: web::Data<AppState>,
    path: web::Path<String>,
    body: web::Json<TriageInput>,
) -> HttpResponse {
    let group_id = path.into_inner();
    let input = body.into_inner();
    let updated_by = req
        .extensions()
        .get::<Authenticated>()
        .and_then(|a| a.user.as_ref().map(|u| u.name.clone()));

    let triage = ExceptionTriage {
        status: input.status,
        note: input.note.filter(|n| !n.trim().is_empty()),
        updated_at: Utc::now(),
        updated_by,
    };

    match state.store.put_triage(&input.project_id, &group_id, &triage) {
        Ok(()) => HttpResponse::NoContent().finish(),
        Err(err) => internal_error(err),
    }
}
