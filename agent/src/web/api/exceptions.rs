//! Exception groups (Sentry-style): the global filterable inbox, group detail,
//! and triage.

use std::collections::HashMap;

use actix_web::http::StatusCode;
use actix_web::{HttpMessage, HttpRequest, HttpResponse, web};
use analytics_api::{
    ExceptionGroupDetail, ExceptionStatus, GlobalException, TriageInput, pixel_source,
};
use chrono::Utc;
use serde::Deserialize;
use tracing_batteries::prelude::*;

use super::query::resolve_range;
use super::{Authenticated, internal_error, json_error};
use crate::analytics::{self, filter::FieldSet};
use crate::state::AppState;

const VARIANT_LIMIT: usize = 50;

/// The triage-store group key for a source-scoped exception group. Triage is
/// per `(project, fingerprint, source)` — the same fingerprint on two
/// applications is two independent failures. The fingerprint is 16 hex chars,
/// so the `@` join is unambiguous.
fn scoped_group(group_id: &str, source: &str) -> String {
    format!("{group_id}@{source}")
}

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

        // One row per (fingerprint, source): a group's identity is the failure
        // *on that application* — the same fingerprint on two sources is two
        // independent rows, each annotated with its owning project (when the
        // source is assigned) for display and triage.
        let mut out: Vec<GlobalException> = Vec::with_capacity(per_source.len());
        for (mut group, source) in per_source {
            let project_id = uri_project.get(&source).cloned();
            let mut project_name = None;
            if let Some(pid) = &project_id {
                if let Some(triage) =
                    store.get_triage(pid, &scoped_group(&group.group_id, &source))?
                {
                    group.resolved = triage.is_resolved(group.last_seen_ms);
                    group.muted = triage.is_muted();
                    group.status = ExceptionStatus::from(&group);
                    group.note = triage.note;
                }
                project_name = project_names.get(pid).cloned();
            }
            out.push(GlobalException {
                group,
                project_id,
                project_name,
                source,
            });
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
    /// The source URI the group was seen on — part of the group's identity,
    /// scoping the detail and its triage state to that one application.
    source: String,
    from: Option<i64>,
    to: Option<i64>,
}

/// `GET /api/v1/exceptions/{group_id}?project=…&source=…` — a group with
/// recent occurrences, scoped to the source it was seen on.
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
    let source = query.source.trim().to_string();
    if source.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "A source is required.");
    }
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
            let sources = [source.clone()];
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
            if let Some(triage) =
                store.get_triage(&project_id, &scoped_group(&group_id, &source))?
            {
                detail.group.resolved = triage.is_resolved(detail.group.last_seen_ms);
                detail.group.muted = triage.is_muted();
                detail.group.status = ExceptionStatus::from(&detail.group);
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

/// `PATCH /api/v1/exceptions/{group_id}` — update a group's triage axes.
///
/// Resolution and suppression are independent: a field left `None` on the input
/// is left unchanged, so the inbox's separate Resolve/Reopen and Mute/Unmute
/// controls each touch only their own axis. Resolving anchors `resolved_at` at
/// now, which is what lets a later occurrence reopen the group automatically.
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

    let source = input.source.trim();
    if source.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "A source is required.");
    }

    match state.store.update_triage(
        &input.project_id,
        &scoped_group(&group_id, source),
        |triage| {
            let now = Utc::now();
            if let Some(resolved) = input.resolved {
                // Anchor resolution at now; reopening clears the anchor.
                triage.resolved_at = resolved.then_some(now);
            }
            if let Some(muted) = input.muted {
                triage.muted_at = muted.then_some(now);
            }
            // A note is only touched when the caller sends one, so toggling an
            // axis never wipes an existing note; an empty note clears it.
            if let Some(note) = input.note {
                triage.note = Some(note).filter(|n| !n.trim().is_empty());
            }
            triage.updated_at = now;
            triage.updated_by = updated_by;
        },
    ) {
        Ok(_) => HttpResponse::NoContent().finish(),
        Err(err) => internal_error(err),
    }
}
