//! Server-driven OIDC session endpoints and the CSRF token endpoint. The agent
//! performs the whole Authorization Code + PKCE exchange so the browser never
//! handles tokens or the client secret. Ported from SierraSoftworks/automate.

use actix_web::cookie::time::Duration as CookieDuration;
use actix_web::cookie::{Cookie, SameSite};
use actix_web::http::StatusCode;
use actix_web::http::header::LOCATION;
use actix_web::{HttpRequest, HttpResponse, web};
use serde::{Deserialize, Serialize};
use tracing_batteries::prelude::*;

use super::{CSRF_COOKIE, OAUTH_COOKIE, SESSION_COOKIE, json_error};
use crate::state::AppState;
use crate::web::extract::{base_url, is_https};
use crate::web::helpers::oidc::{
    authorize_url, discovery, exchange_code, generate_pkce, random_token, validate_token,
};

const DEFAULT_SESSION_SECONDS: i64 = 8 * 60 * 60;
const OAUTH_STATE_SECONDS: i64 = 10 * 60;
const COOKIE_PATH: &str = "/";
const OAUTH_COOKIE_PATH: &str = "/api/v1/auth";

/// Transient state persisted across the redirect to the identity provider.
#[derive(Serialize, Deserialize)]
struct OAuthState {
    state: String,
    verifier: String,
    redirect_uri: String,
    return_to: String,
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginQuery {
    return_to: Option<String>,
}

/// `GET /api/v1/auth/login` — begin the OIDC login by redirecting to the provider.
pub async fn auth_login(
    state: web::Data<AppState>,
    req: HttpRequest,
    query: web::Query<LoginQuery>,
) -> HttpResponse {
    let Some(oidc) = state.config.web.admin.oidc.as_ref() else {
        return redirect_to("/");
    };

    let Some(base) = base_url(&state.config.web, &req) else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "Could not determine the public base URL for the login redirect.",
        );
    };
    let redirect_uri = format!("{base}/api/v1/auth/callback");

    let discovery = match discovery(&state.http, &state.oidc_cache, oidc).await {
        Ok(d) => d,
        Err(err) => {
            error!("Failed to load OIDC discovery document during login: {err}");
            return json_error(
                StatusCode::BAD_GATEWAY,
                "We could not reach the configured identity provider.",
            );
        }
    };

    let pkce = generate_pkce();
    let csrf_state = random_token();
    let authorize = match authorize_url(oidc, &discovery, &redirect_uri, &csrf_state, &pkce.challenge)
    {
        Ok(url) => url,
        Err(err) => {
            error!("Failed to build the OIDC authorization URL: {err}");
            return json_error(
                StatusCode::BAD_GATEWAY,
                "We could not start the sign-in with the identity provider.",
            );
        }
    };

    let oauth_state = OAuthState {
        state: csrf_state,
        verifier: pkce.verifier,
        redirect_uri,
        return_to: sanitize_return_to(query.return_to.as_deref()),
    };
    let Ok(serialized) = serde_json::to_string(&oauth_state) else {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "We could not start the sign-in process.",
        );
    };

    let secure = is_https(state.config.web.trust_proxy, &req);
    let cookie = Cookie::build(OAUTH_COOKIE, serialized)
        .path(OAUTH_COOKIE_PATH)
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(CookieDuration::seconds(OAUTH_STATE_SECONDS))
        .finish();

    HttpResponse::Found()
        .cookie(cookie)
        .insert_header((LOCATION, authorize))
        .finish()
}

/// `GET /api/v1/auth/callback` — complete the exchange and set the session cookie.
pub async fn auth_callback(
    state: web::Data<AppState>,
    req: HttpRequest,
    query: web::Query<CallbackQuery>,
) -> HttpResponse {
    let secure = is_https(state.config.web.trust_proxy, &req);

    let Some(oidc) = state.config.web.admin.oidc.as_ref() else {
        return redirect_to("/");
    };

    if let Some(error) = query.error.as_deref() {
        warn!("The OIDC provider returned an error on the callback: {error}");
        return clear_oauth_and_redirect("/?auth_error=denied");
    }

    let (Some(code), Some(callback_state)) = (query.code.as_deref(), query.state.as_deref()) else {
        return clear_oauth_and_redirect("/?auth_error=invalid");
    };

    let Some(oauth_state) = req
        .cookie(OAUTH_COOKIE)
        .and_then(|c| serde_json::from_str::<OAuthState>(c.value()).ok())
    else {
        return clear_oauth_and_redirect("/?auth_error=expired");
    };

    if oauth_state.state != callback_state {
        warn!("Rejected an OIDC callback whose state did not match the stored value.");
        return clear_oauth_and_redirect("/?auth_error=invalid");
    }

    let discovery = match discovery(&state.http, &state.oidc_cache, oidc).await {
        Ok(d) => d,
        Err(err) => {
            error!("Failed to load OIDC discovery document during callback: {err}");
            return clear_oauth_and_redirect("/?auth_error=provider");
        }
    };

    let id_token = match exchange_code(
        &state.http,
        oidc,
        &discovery,
        code,
        &oauth_state.verifier,
        &oauth_state.redirect_uri,
    )
    .await
    {
        Ok(token) => token,
        Err(err) => {
            warn!("OIDC token exchange failed: {err}");
            return clear_oauth_and_redirect("/?auth_error=exchange");
        }
    };

    let claims = match validate_token(&state.http, &state.oidc_cache, oidc, &id_token).await {
        Ok(claims) => claims,
        Err(err) => {
            warn!("OIDC provider issued an ID token that failed validation: {err}");
            return clear_oauth_and_redirect("/?auth_error=token");
        }
    };

    let max_age = claims
        .get("exp")
        .and_then(|v| v.as_i64())
        .map(|exp| exp - chrono::Utc::now().timestamp())
        .filter(|secs| *secs > 0)
        .unwrap_or(DEFAULT_SESSION_SECONDS);

    let session_cookie = Cookie::build(SESSION_COOKIE, id_token)
        .path(COOKIE_PATH)
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(CookieDuration::seconds(max_age))
        .finish();

    let mut oauth_removal = Cookie::build(OAUTH_COOKIE, "")
        .path(OAUTH_COOKIE_PATH)
        .finish();
    oauth_removal.make_removal();

    HttpResponse::Found()
        .cookie(session_cookie)
        .cookie(oauth_removal)
        .insert_header((LOCATION, oauth_state.return_to))
        .finish()
}

/// `POST /api/v1/auth/logout` — clear the session cookie.
pub async fn auth_logout() -> HttpResponse {
    let mut removal = Cookie::build(SESSION_COOKIE, "").path(COOKIE_PATH).finish();
    removal.make_removal();
    HttpResponse::NoContent().cookie(removal).finish()
}

/// `GET /api/v1/csrf` — issue a double-submit CSRF token (body + matching cookie).
pub async fn csrf_token(state: web::Data<AppState>, req: HttpRequest) -> HttpResponse {
    let secure = is_https(state.config.web.trust_proxy, &req);
    let token = random_token();
    let cookie = Cookie::build(CSRF_COOKIE, token.clone())
        .path(COOKIE_PATH)
        // Deliberately NOT HttpOnly: the SPA reads it to echo in X-CSRF-Token.
        .http_only(false)
        .secure(secure)
        .same_site(SameSite::Lax)
        .finish();
    HttpResponse::Ok()
        .cookie(cookie)
        .json(analytics_api::CsrfToken { token })
}

fn redirect_to(location: &str) -> HttpResponse {
    HttpResponse::Found()
        .insert_header((LOCATION, location))
        .finish()
}

fn clear_oauth_and_redirect(location: &str) -> HttpResponse {
    let mut removal = Cookie::build(OAUTH_COOKIE, "")
        .path(OAUTH_COOKIE_PATH)
        .finish();
    removal.make_removal();
    HttpResponse::Found()
        .cookie(removal)
        .insert_header((LOCATION, location))
        .finish()
}

/// Ensure the post-login destination is a safe, same-site relative path.
fn sanitize_return_to(value: Option<&str>) -> String {
    match value {
        Some(path)
            if path.starts_with('/') && !path.starts_with("//") && !path.starts_with("/\\") =>
        {
            path.to_string()
        }
        _ => "/".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_return_to_accepts_local_paths() {
        assert_eq!(sanitize_return_to(Some("/projects/abc")), "/projects/abc");
    }

    #[test]
    fn sanitize_return_to_rejects_external_destinations() {
        assert_eq!(sanitize_return_to(None), "/");
        assert_eq!(sanitize_return_to(Some("https://evil.example")), "/");
        assert_eq!(sanitize_return_to(Some("//evil.example")), "/");
        assert_eq!(sanitize_return_to(Some("/\\evil")), "/");
        assert_eq!(sanitize_return_to(Some("relative")), "/");
    }
}
