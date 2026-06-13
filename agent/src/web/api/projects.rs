//! Project CRUD and per-project statistics.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, web};
use analytics_api::{Project, ProjectInput};
use chrono::Utc;
use tracing_batteries::prelude::*;

use super::query::{StatsQuery, resolve_range, subset};
use super::{internal_error, json_error};
use crate::analytics;
use crate::state::AppState;

pub async fn list(state: web::Data<AppState>) -> HttpResponse {
    match state.store.list_projects() {
        Ok(mut projects) => {
            projects.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            HttpResponse::Ok().json(projects)
        }
        Err(err) => internal_error(err),
    }
}

pub async fn create(state: web::Data<AppState>, body: web::Json<ProjectInput>) -> HttpResponse {
    let input = body.into_inner();
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "A project name is required.");
    }
    let project = Project {
        id: ulid::Ulid::new().to_string(),
        slug: input.slug.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| slugify(&name)),
        name,
        created_at: Utc::now(),
    };
    match state.store.put_project(&project) {
        Ok(()) => HttpResponse::Created().json(project),
        Err(err) => internal_error(err),
    }
}

pub async fn get(state: web::Data<AppState>, path: web::Path<String>) -> HttpResponse {
    match state.store.get_project(&path.into_inner()) {
        Ok(Some(project)) => HttpResponse::Ok().json(project),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "Project not found."),
        Err(err) => internal_error(err),
    }
}

pub async fn update(
    state: web::Data<AppState>,
    path: web::Path<String>,
    body: web::Json<ProjectInput>,
) -> HttpResponse {
    let id = path.into_inner();
    let existing = match state.store.get_project(&id) {
        Ok(Some(project)) => project,
        Ok(None) => return json_error(StatusCode::NOT_FOUND, "Project not found."),
        Err(err) => return internal_error(err),
    };
    let input = body.into_inner();
    let name = input.name.trim().to_string();
    let updated = Project {
        slug: input.slug.filter(|s| !s.trim().is_empty()).unwrap_or(existing.slug),
        name: if name.is_empty() { existing.name } else { name },
        ..existing
    };
    match state.store.put_project(&updated) {
        Ok(()) => HttpResponse::Ok().json(updated),
        Err(err) => internal_error(err),
    }
}

pub async fn delete(state: web::Data<AppState>, path: web::Path<String>) -> HttpResponse {
    match state.store.delete_project(&path.into_inner()) {
        Ok(true) => HttpResponse::NoContent().finish(),
        Ok(false) => json_error(StatusCode::NOT_FOUND, "Project not found."),
        Err(err) => internal_error(err),
    }
}

pub async fn stats(
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<StatsQuery>,
) -> HttpResponse {
    let project_id = path.into_inner();
    match state.store.get_project(&project_id) {
        Ok(Some(_)) => {}
        Ok(None) => return json_error(StatusCode::NOT_FOUND, "Project not found."),
        Err(err) => return internal_error(err),
    }

    let (from, to, bucket) = resolve_range(&query);
    let subset = subset(&query);
    let store = state.store.clone();
    let parquet_dir = state.config.storage.parquet_dir.clone();

    let result = web::block(move || {
        let mut sources = analytics::project_source_uris(&store, &project_id)?;
        if let Some(subset) = subset {
            sources.retain(|s| subset.contains(s));
        }
        analytics::stats_for_sources(&store, &parquet_dir, &sources, from, to, bucket)
    })
    .await;

    match result {
        Ok(Ok(stats)) => HttpResponse::Ok().json(stats),
        Ok(Err(err)) => internal_error(err),
        Err(err) => {
            error!("stats computation task failed: {err}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to compute statistics.")
        }
    }
}

/// Lower-case, hyphenated slug derived from a name.
fn slugify(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut last_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() { "project".to_string() } else { slug }
}
