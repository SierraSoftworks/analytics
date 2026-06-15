//! The signed-in administrator's identity.

use actix_web::{HttpMessage, HttpRequest, HttpResponse};

use super::Authenticated;

/// `GET /api/v1/me` — the signed-in administrator's display identity. When OIDC is
/// disabled there is no authenticated user, so this responds `204 No Content`.
pub async fn me(req: HttpRequest) -> HttpResponse {
    let user = req
        .extensions()
        .get::<Authenticated>()
        .and_then(|a| a.user.clone());
    match user {
        Some(user) => HttpResponse::Ok().json(user),
        None => HttpResponse::NoContent().finish(),
    }
}
