//! Exception groups (Sentry-style): per-project listing, group detail, and triage.

use std::collections::HashMap;

use actix_web::http::StatusCode;
use actix_web::{HttpMessage, HttpRequest, HttpResponse, web};
use analytics_api::{ExceptionGroupDetail, GlobalException, TriageInput, pixel_source};
use chrono::Utc;
use serde::Deserialize;
use tracing_batteries::prelude::*;

use super::query::{StatsQuery, resolve_range};
use super::{Authenticated, internal_error, json_error};
use crate::analytics;
use crate::state::AppState;
use crate::store::ExceptionTriage;

const OCCURRENCE_LIMIT: usize = 50;

/// `GET /api/v1/exceptions` — exception groups across every project (and
/// unassigned sources), each annotated with its project, for the global inbox.
pub async fn list_all(state: web::Data<AppState>, query: web::Query<StatsQuery>) -> HttpResponse {
    let (from, to, _) = resolve_range(&query);
    let store = state.store.clone();
    let parquet_dir = state.config.storage.parquet_dir.clone();

    let result = web::block(move || -> crate::errors::Result<Vec<GlobalException>> {
        let per_source = analytics::global_exception_groups(&store, &parquet_dir, from, to)?;

        // Resolve a source URI to its owning project, and project ids to names.
        let mut uri_project: HashMap<String, String> = HashMap::new();
        for source in store.list_sources()? {
            if let Some(project_id) = source.project_id {
                uri_project.insert(source.uri, project_id);
            }
        }
        for pixel in store.list_pixels()? {
            uri_project.insert(pixel_source(&pixel.id), pixel.project_id);
        }
        let project_names: HashMap<String, String> =
            store.list_projects()?.into_iter().map(|p| (p.id, p.name)).collect();

        // Fold per-(fingerprint, source) rows up to per-(fingerprint, project) rows
        // so a project's count matches its detail page. Unassigned sources are keyed
        // by source so each stays its own row (no project to merge into).
        use std::collections::hash_map::Entry;
        let mut acc: HashMap<(String, String), GlobalException> = HashMap::new();
        for (group, source) in per_source {
            let project_id = uri_project.get(&source).cloned();
            let bucket = project_id.clone().unwrap_or_else(|| format!("@{source}"));
            let key = (group.group_id.clone(), bucket);
            match acc.entry(key) {
                Entry::Occupied(mut e) => {
                    let g = &mut e.get_mut().group;
                    g.count += group.count;
                    g.first_seen_ms = g.first_seen_ms.min(group.first_seen_ms);
                    g.last_seen_ms = g.last_seen_ms.max(group.last_seen_ms);
                }
                Entry::Vacant(e) => {
                    e.insert(GlobalException { group, project_id, project_name: None, source });
                }
            }
        }

        let mut out: Vec<GlobalException> = Vec::with_capacity(acc.len());
        for (_, mut item) in acc {
            if let Some(pid) = &item.project_id {
                if let Some(triage) = store.get_triage(pid, &item.group.group_id)? {
                    item.group.status = triage.status;
                    item.group.note = triage.note;
                }
                item.project_name = project_names.get(pid).cloned();
            }
            out.push(item);
        }
        out.sort_by_key(|e| std::cmp::Reverse(e.group.last_seen_ms));
        Ok(out)
    })
    .await;

    match result {
        Ok(Ok(groups)) => HttpResponse::Ok().json(groups),
        Ok(Err(err)) => internal_error(err),
        Err(err) => {
            error!("global exception listing task failed: {err}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to load exceptions.")
        }
    }
}

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
        let Some((mut group, occurrences)) = analytics::exception_detail(
            &store,
            &parquet_dir,
            &sources,
            &group_id,
            from,
            to,
            OCCURRENCE_LIMIT,
        )?
        else {
            return Ok(None);
        };
        if let Some(triage) = store.get_triage(&project_id, &group_id)? {
            group.status = triage.status;
            group.note = triage.note;
        }
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
