//! Tracking-pixel (GIF) management. Pixels belong to a project; the public GIF
//! endpoint (`/track/gif/{id}.gif`) records a hit for the resolved pixel.

use actix_web::http::StatusCode;
use actix_web::{HttpResponse, web};
use analytics_api::{Pixel, PixelInput};
use chrono::Utc;

use super::{internal_error, json_error};
use crate::state::AppState;

pub async fn list(state: web::Data<AppState>, path: web::Path<String>) -> HttpResponse {
    let project_id = path.into_inner();
    match state.store.list_pixels() {
        Ok(pixels) => {
            let mut pixels: Vec<Pixel> =
                pixels.into_iter().filter(|p| p.project_id == project_id).collect();
            pixels.sort_by_key(|p| p.name.to_lowercase());
            HttpResponse::Ok().json(pixels)
        }
        Err(err) => internal_error(err),
    }
}

/// `GET /api/v1/pixels` — every pixel across all projects, for the global Tracking
/// Pixels page. Each pixel carries its `project_id` so the UI can label it.
pub async fn list_all(state: web::Data<AppState>) -> HttpResponse {
    match state.store.list_pixels() {
        Ok(mut pixels) => {
            pixels.sort_by_key(|p| p.name.to_lowercase());
            HttpResponse::Ok().json(pixels)
        }
        Err(err) => internal_error(err),
    }
}

pub async fn create(
    state: web::Data<AppState>,
    path: web::Path<String>,
    body: web::Json<PixelInput>,
) -> HttpResponse {
    let project_id = path.into_inner();
    match state.store.get_project(&project_id) {
        Ok(Some(_)) => {}
        Ok(None) => return json_error(StatusCode::NOT_FOUND, "Project not found."),
        Err(err) => return internal_error(err),
    }

    let input = body.into_inner();
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "A pixel name is required.");
    }
    let pixel = Pixel {
        id: ulid::Ulid::new().to_string(),
        project_id,
        name,
        event_name: input
            .event_name
            .filter(|e| !e.trim().is_empty())
            .unwrap_or_else(|| "pixel".to_string()),
        metadata: input.metadata,
        created_at: Utc::now(),
        last_hit: None,
    };
    match state.store.put_pixel(&pixel) {
        Ok(()) => HttpResponse::Created().json(pixel),
        Err(err) => internal_error(err),
    }
}

pub async fn get(state: web::Data<AppState>, path: web::Path<String>) -> HttpResponse {
    match state.store.get_pixel(&path.into_inner()) {
        Ok(Some(pixel)) => HttpResponse::Ok().json(pixel),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "Pixel not found."),
        Err(err) => internal_error(err),
    }
}

pub async fn update(
    state: web::Data<AppState>,
    path: web::Path<String>,
    body: web::Json<PixelInput>,
) -> HttpResponse {
    let existing = match state.store.get_pixel(&path.into_inner()) {
        Ok(Some(pixel)) => pixel,
        Ok(None) => return json_error(StatusCode::NOT_FOUND, "Pixel not found."),
        Err(err) => return internal_error(err),
    };
    let input = body.into_inner();
    let name = input.name.trim().to_string();
    let updated = Pixel {
        name: if name.is_empty() { existing.name } else { name },
        event_name: input
            .event_name
            .filter(|e| !e.trim().is_empty())
            .unwrap_or(existing.event_name),
        metadata: input.metadata,
        ..existing
    };
    match state.store.put_pixel(&updated) {
        Ok(()) => HttpResponse::Ok().json(updated),
        Err(err) => internal_error(err),
    }
}

pub async fn delete(state: web::Data<AppState>, path: web::Path<String>) -> HttpResponse {
    match state.store.delete_pixel(&path.into_inner()) {
        Ok(true) => HttpResponse::NoContent().finish(),
        Ok(false) => json_error(StatusCode::NOT_FOUND, "Pixel not found."),
        Err(err) => internal_error(err),
    }
}
