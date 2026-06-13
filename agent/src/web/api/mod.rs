//! The JSON REST API consumed by the dashboard.
//!
//! Everything under `/api/v1` except `/health` and `/auth/*` is gated by
//! [`api_auth`], which authenticates the session cookie (when OIDC is configured),
//! evaluates the admin ACL (filt-rs), enforces a double-submit CSRF check on
//! mutating requests, and rate-limits unauthenticated callers by IP.

mod auth;
mod exceptions;
mod me;
mod overview;
mod pixels;
mod projects;
mod query;
mod sources;

use actix_web::http::StatusCode;
use actix_web::{
    HttpResponse,
    body::BoxBody,
    cookie::Cookie,
    dev::{ServiceRequest, ServiceResponse},
    http::Method,
    middleware::{Next, from_fn},
    web,
};
use tracing_batteries::prelude::*;

use crate::state::AppState;
use crate::web::helpers::oidc::{
    AdminRequestFilter, admin_user_from_claims, filterable_claims, validate_token,
};

/// The `HttpOnly` cookie holding the signed-in administrator's OIDC ID token.
pub const SESSION_COOKIE: &str = "analytics_session";
/// The non-`HttpOnly` cookie holding the double-submit CSRF token.
pub const CSRF_COOKIE: &str = "analytics_csrf";
/// The short-lived cookie holding in-flight OAuth state during login.
pub const OAUTH_COOKIE: &str = "analytics_oauth";
/// The header the browser echoes the CSRF token back in on mutating requests.
const CSRF_HEADER: &str = "x-csrf-token";

/// The validated identity attached to a request after authentication.
#[derive(Clone)]
pub struct Authenticated {
    pub user: Option<analytics_api::AdminUser>,
}

/// Build a JSON error response.
pub fn json_error(status: StatusCode, message: impl ToString) -> HttpResponse {
    HttpResponse::build(status).json(serde_json::json!({ "error": message.to_string() }))
}

/// Register the `/api/v1` routes: public `/health` + `/auth/*`, everything else
/// behind [`api_auth`]. Phases 6-7 extend the protected scope.
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/v1")
            .route("/health", web::get().to(health))
            .service(
                web::scope("/auth")
                    .route("/login", web::get().to(auth::auth_login))
                    .route("/callback", web::get().to(auth::auth_callback))
                    .route("/logout", web::post().to(auth::auth_logout)),
            )
            .service(
                web::scope("")
                    .wrap(from_fn(api_auth))
                    .route("/csrf", web::get().to(auth::csrf_token))
                    .route("/me", web::get().to(me::me))
                    .route("/overview", web::get().to(overview::overview))
                    .route("/projects", web::get().to(projects::list))
                    .route("/projects", web::post().to(projects::create))
                    .route("/projects/{id}", web::get().to(projects::get))
                    .route("/projects/{id}", web::put().to(projects::update))
                    .route("/projects/{id}", web::delete().to(projects::delete))
                    .route("/projects/{id}/stats", web::get().to(projects::stats))
                    .route("/sources", web::get().to(sources::list))
                    .route("/sources", web::put().to(sources::update))
                    .route("/sources", web::delete().to(sources::delete))
                    .route("/projects/{id}/pixels", web::get().to(pixels::list))
                    .route("/projects/{id}/pixels", web::post().to(pixels::create))
                    .route("/pixels/{id}", web::get().to(pixels::get))
                    .route("/pixels/{id}", web::put().to(pixels::update))
                    .route("/pixels/{id}", web::delete().to(pixels::delete))
                    .route("/projects/{id}/exceptions", web::get().to(exceptions::list))
                    .route("/exceptions/{group}", web::get().to(exceptions::detail))
                    .route("/exceptions/{group}", web::patch().to(exceptions::triage)),
            ),
    );
}

/// Log an internal error and return a generic 500 (details stay server-side).
pub fn internal_error(err: human_errors::Error) -> HttpResponse {
    error!("internal API error: {err}");
    json_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "An internal error occurred.",
    )
}

async fn health() -> HttpResponse {
    HttpResponse::Ok().json(analytics_api::Health { ok: true })
}

fn is_mutating(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// Authentication + authorisation middleware for the protected API.
pub async fn api_auth(
    req: ServiceRequest,
    next: Next<BoxBody>,
) -> Result<ServiceResponse<BoxBody>, actix_web::Error> {
    use actix_web::HttpMessage;

    let Some(state) = req.app_data::<web::Data<AppState>>().cloned() else {
        return Ok(req.into_response(json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Service context unavailable.",
        )));
    };

    // Reject forged cross-site mutations early.
    if is_mutating(req.method()) && !csrf_ok(&req) {
        return Ok(req.into_response(json_error(
            StatusCode::FORBIDDEN,
            "The request could not be verified. Please refresh the page and try again.",
        )));
    }

    // Authenticate via the session cookie when OIDC is configured.
    let claims = if let Some(oidc) = &state.config.web.admin.oidc {
        match req.cookie(SESSION_COOKIE).map(|c| c.value().to_string()) {
            None => {
                let response = unauthenticated(&state, &req);
                return Ok(req.into_response(response));
            }
            Some(token) => match validate_token(&state.http, &state.oidc_cache, oidc, &token).await {
                Ok(claims) => Some(claims),
                Err(err) => {
                    info!("Rejected API request with an invalid session cookie: {err}");
                    let response = unauthenticated(&state, &req);
                    return Ok(req.into_response(response));
                }
            },
        }
    } else {
        None
    };

    let filterable = claims.as_ref().map(filterable_claims);
    let filter = AdminRequestFilter {
        method: req.method().as_str(),
        path: req.path(),
        client_ip: req.peer_addr().map(|addr| addr.ip().to_string()),
        headers: req.headers(),
        claims: filterable.as_ref(),
    };

    if !state.acl.matches(&filter).unwrap_or(false) {
        // Authenticated-but-forbidden is a 403; an unauthenticated denial is
        // throttled like any other unauthenticated probe.
        if claims.is_some() {
            return Ok(req.into_response(json_error(
                StatusCode::FORBIDDEN,
                "Your account is not permitted to access this resource.",
            )));
        }
        let response = unauthenticated(&state, &req);
        return Ok(req.into_response(response));
    }

    let user = claims.as_ref().map(admin_user_from_claims);
    req.extensions_mut().insert(Authenticated { user });
    next.call(req).await
}

/// Build the response for an unauthenticated request, throttling repeat offenders
/// by IP (the IP is a transient limiter key only — never stored or logged).
fn unauthenticated(state: &AppState, req: &ServiceRequest) -> HttpResponse {
    if state.config.ratelimit.enabled {
        let ip = client_ip(req, state.config.web.trust_proxy);
        if !state.unauth_limiter.check(&ip) {
            return json_error(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many unauthenticated requests. Please slow down.",
            );
        }
    }
    json_error(
        StatusCode::UNAUTHORIZED,
        "Authentication is required to access this resource.",
    )
}

fn client_ip(req: &ServiceRequest, trust_proxy: bool) -> String {
    if trust_proxy {
        if let Some(forwarded) = req.headers().get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            if let Some(first) = forwarded.split(',').next() {
                let ip = first.trim();
                if !ip.is_empty() {
                    return ip.to_string();
                }
            }
        }
        if let Some(real) = req.headers().get("x-real-ip").and_then(|v| v.to_str().ok()) {
            let ip = real.trim();
            if !ip.is_empty() {
                return ip.to_string();
            }
        }
    }
    req.peer_addr()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_default()
}

/// Double-submit CSRF check: the `X-CSRF-Token` header must equal the CSRF cookie.
fn csrf_ok(req: &ServiceRequest) -> bool {
    let header = req
        .headers()
        .get(CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let cookie = req.cookie(CSRF_COOKIE).map(|c: Cookie| c.value().to_string());
    match (header, cookie) {
        (Some(header), Some(cookie)) => !header.is_empty() && header == cookie,
        _ => false,
    }
}
