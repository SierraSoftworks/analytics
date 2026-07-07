//! Exception groups (Sentry-style): the global filterable inbox, group detail,
//! and triage.

use std::collections::HashMap;

use actix_web::http::StatusCode;
use actix_web::{HttpMessage, HttpRequest, HttpResponse, web};
use analytics_api::{ExceptionGroupDetail, GlobalException, TriageInput, pixel_source};
use chrono::Utc;
use serde::Deserialize;
use tracing_batteries::prelude::*;

use super::query::resolve_range;
use super::{Authenticated, internal_error, json_error};
use crate::analytics::{self, filter::FieldSet};
use crate::state::AppState;
use crate::store::ExceptionTriage;

const VARIANT_LIMIT: usize = 50;

/// Query parameters for the global exceptions inbox: a time range plus a
/// filt-rs `q` expression over the dimensions exception events carry
/// (project, source, browser, os, device, app, app_version, type, message,
/// handled).
#[derive(Deserialize)]
pub struct ExceptionsQuery {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub q: Option<String>,
}

/// `GET /api/v1/exceptions` — exception groups across every project (and
/// unassigned sources), each annotated with its project, for the global inbox.
pub async fn list_all(
    state: web::Data<AppState>,
    query: web::Query<ExceptionsQuery>,
) -> HttpResponse {
    let query = query.into_inner();
    let store = state.store.clone();
    let parquet_dir = state.config.storage.parquet_dir.clone();

    let filter = match query.q.as_deref() {
        Some(q) => match analytics::filter::compile_query(q, FieldSet::Exceptions, &store) {
            Ok(filter) => filter,
            Err(message) => return json_error(StatusCode::BAD_REQUEST, message),
        },
        None => None,
    };

    let result = web::block(move || -> crate::errors::Result<Vec<GlobalException>> {
        // `from=0` means "all time": anchor at the earliest stored event so the
        // per-group trend buckets cover the data, not decades of empty space.
        let from = match query.from {
            Some(f) if f <= 0 => analytics::earliest_event_ms(&store, &parquet_dir)?,
            other => other,
        };
        let (from, to, _) = resolve_range(from, query.to, None);
        let per_source =
            analytics::exception_groups_by_source(&store, &parquet_dir, from, to, filter.as_ref())?;

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
        let project_names: HashMap<String, String> = store
            .list_projects()?
            .into_iter()
            .map(|p| (p.id, p.name))
            .collect();

        // Fold per-(fingerprint, source) rows up to per-(fingerprint, project) rows
        // so a project's count matches its detail page. Unassigned sources are keyed
        // by source so each stays its own row (no project to merge into). Trends are
        // summed element-wise — every row shares the same [from, to) bucket grid.
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
                    analytics::merge_trends(&mut g.trend, &group.trend);
                }
                Entry::Vacant(e) => {
                    e.insert(GlobalException {
                        group,
                        project_id,
                        project_name: None,
                        source,
                    });
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
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to load exceptions.",
            )
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
///
/// Without an explicit range this looks across **all time**: a group linked
/// from the inbox (which may cover a 12-month window) or from an old bookmark
/// must always open, so it can be triaged.
pub async fn detail(
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<DetailQuery>,
) -> HttpResponse {
    let group_id = path.into_inner();
    let project_id = query.project.clone();
    let now = Utc::now().timestamp_millis();
    let to = query
        .to
        .unwrap_or(now)
        .clamp(1, super::query::MAX_INSTANT_MS);
    let from = query.from.unwrap_or(0).clamp(0, to - 1);
    let store = state.store.clone();
    let parquet_dir = state.config.storage.parquet_dir.clone();

    let result = web::block(
        move || -> crate::errors::Result<Option<ExceptionGroupDetail>> {
            let sources = analytics::project_source_uris(&store, &project_id)?;
            let Some(mut detail) = analytics::exception_detail(
                &store,
                &parquet_dir,
                &sources,
                &group_id,
                from,
                to,
                VARIANT_LIMIT,
            )?
            else {
                return Ok(None);
            };
            if let Some(triage) = store.get_triage(&project_id, &group_id)? {
                detail.group.status = triage.status;
                detail.group.note = triage.note;
            }
            Ok(Some(detail))
        },
    )
    .await;

    match result {
        Ok(Ok(Some(detail))) => HttpResponse::Ok().json(detail),
        Ok(Ok(None)) => json_error(StatusCode::NOT_FOUND, "Exception group not found."),
        Ok(Err(err)) => internal_error(err),
        Err(err) => {
            error!("exception detail task failed: {err}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to load the exception.",
            )
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

    match state
        .store
        .put_triage(&input.project_id, &group_id, &triage)
    {
        Ok(()) => HttpResponse::NoContent().finish(),
        Err(err) => internal_error(err),
    }
}
