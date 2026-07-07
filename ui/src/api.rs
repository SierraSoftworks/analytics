//! Thin client over the agent's `/api/v1` REST endpoints.
//!
//! Auth relies on the agent's `HttpOnly` session cookie (sent automatically on
//! same-origin requests); the UI never sees a token. Mutating requests carry a
//! double-submit CSRF token in `X-CSRF-Token`, fetched once and cached. A `401`
//! first triggers a transparent session renewal ([`crate::auth::refresh_session`])
//! and a single retry; only when the session truly cannot be renewed does it
//! surface as [`ApiError::Unauthorized`] so callers can redirect to login.

use std::cell::RefCell;

use analytics_api::{
    AdminUser, CsrfToken, Dashboard, EventDetail, ExceptionGroupDetail, GlobalException, Instance,
    Pixel, PixelInput, Project, ProjectInput, SessionTrace, Source, SourceInput, TriageInput,
};
use gloo_net::http::Request;
use serde::Serialize;
use serde::de::DeserializeOwned;

const API_BASE: &str = "/api/v1";
const CSRF_HEADER: &str = "X-CSRF-Token";

thread_local! {
    static CSRF_TOKEN: RefCell<Option<String>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    Unauthorized,
    Forbidden,
    Network(String),
    Server(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Unauthorized => write!(f, "Your session has expired. Please sign in again."),
            ApiError::Forbidden => write!(f, "You are not permitted to perform this action."),
            ApiError::Network(msg) => write!(f, "Network error: {msg}"),
            ApiError::Server(msg) => write!(f, "{msg}"),
        }
    }
}

#[derive(serde::Deserialize)]
struct ServerError {
    error: String,
}

fn net<E: ToString>(e: E) -> ApiError {
    ApiError::Network(e.to_string())
}

async fn error_from(resp: gloo_net::http::Response) -> ApiError {
    match resp.status() {
        401 => ApiError::Unauthorized,
        403 => ApiError::Forbidden,
        status => match resp.json::<ServerError>().await {
            Ok(body) => ApiError::Server(body.error),
            Err(_) => ApiError::Server(format!("The server returned an error ({status}).")),
        },
    }
}

fn cached_csrf() -> Option<String> {
    CSRF_TOKEN.with(|t| t.borrow().clone())
}

async fn fetch_csrf() -> Result<String, ApiError> {
    let url = format!("{API_BASE}/csrf");
    let resp = send_with_session(|| Request::get(&url).build()).await?;
    if !resp.ok() {
        return Err(error_from(resp).await);
    }
    let token = resp.json::<CsrfToken>().await.map_err(net)?.token;
    CSRF_TOKEN.with(|t| *t.borrow_mut() = Some(token.clone()));
    Ok(token)
}

async fn ensure_csrf() -> Result<String, ApiError> {
    match cached_csrf() {
        Some(token) => Ok(token),
        None => fetch_csrf().await,
    }
}

fn invalidate_csrf() {
    CSRF_TOKEN.with(|t| *t.borrow_mut() = None);
}

/// Send a request, transparently renewing the session and retrying once when the
/// server rejects it with a `401` (the session cookie lapsed, but the agent may
/// hold a refresh token it can redeem). The renewal is coalesced across
/// concurrent callers, so a page's parallel fetches failing together produce a
/// single refresh. When renewal fails the original `401` is returned untouched.
async fn send_with_session<F>(build: F) -> Result<gloo_net::http::Response, ApiError>
where
    F: Fn() -> Result<Request, gloo_net::Error>,
{
    let resp = build().map_err(net)?.send().await.map_err(net)?;
    if resp.status() != 401 {
        return Ok(resp);
    }
    if crate::auth::refresh_session().await.is_err() {
        return Ok(resp);
    }
    build().map_err(net)?.send().await.map_err(net)
}

async fn get_json<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
    let url = format!("{API_BASE}{path}");
    let resp = send_with_session(|| Request::get(&url).build()).await?;
    if !resp.ok() {
        return Err(error_from(resp).await);
    }
    resp.json::<T>().await.map_err(net)
}

/// Send a mutating request (with CSRF), retrying once on a stale-token `403`.
async fn mutate<F>(build: F) -> Result<gloo_net::http::Response, ApiError>
where
    F: Fn(&str) -> Result<Request, gloo_net::Error>,
{
    let mut refreshed = false;
    loop {
        let token = ensure_csrf().await?;
        let resp = send_with_session(|| build(&token)).await?;
        if resp.ok() {
            return Ok(resp);
        }
        if resp.status() == 403 && !refreshed {
            invalidate_csrf();
            fetch_csrf().await?;
            refreshed = true;
            continue;
        }
        return Err(error_from(resp).await);
    }
}

async fn post_json<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> Result<T, ApiError> {
    let url = format!("{API_BASE}{path}");
    let resp = mutate(|token| Request::post(&url).header(CSRF_HEADER, token).json(body)).await?;
    resp.json::<T>().await.map_err(net)
}

async fn put_json<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> Result<T, ApiError> {
    let url = format!("{API_BASE}{path}");
    let resp = mutate(|token| Request::put(&url).header(CSRF_HEADER, token).json(body)).await?;
    resp.json::<T>().await.map_err(net)
}

async fn patch_empty<B: Serialize>(path: &str, body: &B) -> Result<(), ApiError> {
    let url = format!("{API_BASE}{path}");
    mutate(|token| Request::patch(&url).header(CSRF_HEADER, token).json(body)).await?;
    Ok(())
}

async fn delete(path: &str) -> Result<(), ApiError> {
    let url = format!("{API_BASE}{path}");
    mutate(|token| Request::delete(&url).header(CSRF_HEADER, token).build()).await?;
    Ok(())
}

fn enc(value: &str) -> String {
    js_sys::encode_uri_component(value).into()
}

// ------------------------------------------------------------------ endpoints

pub async fn me() -> Result<Option<AdminUser>, ApiError> {
    let url = format!("{API_BASE}/me");
    let resp = send_with_session(|| Request::get(&url).build()).await?;
    if resp.status() == 204 {
        return Ok(None);
    }
    if !resp.ok() {
        return Err(error_from(resp).await);
    }
    resp.json::<AdminUser>().await.map(Some).map_err(net)
}

pub async fn logout() -> Result<(), ApiError> {
    mutate(|token| {
        Request::post(&format!("{API_BASE}/auth/logout"))
            .header(CSRF_HEADER, token)
            .build()
    })
    .await
    .map(|_| ())
}

/// The full dashboard payload for a resolved filter query (see
/// [`crate::filters::FilterSet::stats_query`]).
pub async fn dashboard(query: &str) -> Result<Dashboard, ApiError> {
    get_json(&format!("/stats?{query}")).await
}

pub async fn instance() -> Result<Instance, ApiError> {
    get_json("/instance").await
}

/// Every pixel across all projects (for the global Tracking Pixels page).
pub async fn list_all_pixels() -> Result<Vec<Pixel>, ApiError> {
    get_json("/pixels").await
}

pub async fn list_projects() -> Result<Vec<Project>, ApiError> {
    get_json("/projects").await
}

pub async fn create_project(input: &ProjectInput) -> Result<Project, ApiError> {
    post_json("/projects", input).await
}

pub async fn delete_project(id: &str) -> Result<(), ApiError> {
    delete(&format!("/projects/{}", enc(id))).await
}

pub async fn get_project(id: &str) -> Result<Project, ApiError> {
    get_json(&format!("/projects/{}", enc(id))).await
}

pub async fn update_project(id: &str, input: &ProjectInput) -> Result<Project, ApiError> {
    put_json(&format!("/projects/{}", enc(id)), input).await
}

pub async fn list_sources() -> Result<Vec<Source>, ApiError> {
    get_json("/sources").await
}

pub async fn update_source(uri: &str, input: &SourceInput) -> Result<Source, ApiError> {
    put_json(&format!("/sources?uri={}", enc(uri)), input).await
}

pub async fn create_pixel(project_id: &str, input: &PixelInput) -> Result<Pixel, ApiError> {
    post_json(&format!("/projects/{}/pixels", enc(project_id)), input).await
}

pub async fn update_pixel(id: &str, input: &PixelInput) -> Result<Pixel, ApiError> {
    put_json(&format!("/pixels/{}", enc(id)), input).await
}

pub async fn delete_pixel(id: &str) -> Result<(), ApiError> {
    delete(&format!("/pixels/{}", enc(id))).await
}

/// Exception groups across every project (and unassigned sources), filtered by
/// a resolved query (see [`crate::filters::FilterSet::exceptions_query`]).
pub async fn list_all_exceptions(query: &str) -> Result<Vec<GlobalException>, ApiError> {
    get_json(&format!("/exceptions?{query}")).await
}

/// One custom/pixel event in detail. `query` is a pre-encoded dashboard query
/// (range + `q`) so the numbers cover the same slice as the panel that linked
/// here.
pub async fn event_detail(name: &str, query: &str) -> Result<EventDetail, ApiError> {
    let mut url = format!("/events?name={}", enc(name));
    if !query.is_empty() {
        url.push('&');
        url.push_str(query);
    }
    get_json(&url).await
}

/// `range` is a pre-encoded `from=…&to=…` pair (empty for the server's
/// all-time default) so the detail numbers cover the same window as the inbox
/// row the operator clicked. `source` scopes the group to the application it
/// was seen on (`None` only on pre-source deep links).
pub async fn exception_detail(
    group: &str,
    project: &str,
    source: Option<&str>,
    range: &str,
) -> Result<ExceptionGroupDetail, ApiError> {
    let mut url = format!("/exceptions/{}?project={}", enc(group), enc(project));
    if let Some(source) = source {
        url.push_str(&format!("&source={}", enc(source)));
    }
    if !range.is_empty() {
        url.push('&');
        url.push_str(range);
    }
    get_json(&url).await
}

pub async fn set_triage(group: &str, input: &TriageInput) -> Result<(), ApiError> {
    patch_empty(&format!("/exceptions/{}", enc(group)), input).await
}

/// One session's full event timeline. No range is passed: a trace linked from
/// the dashboard sample or an exception exemplar must always open whole.
pub async fn session_trace(id: &str) -> Result<SessionTrace, ApiError> {
    get_json(&format!("/traces/{}", enc(id))).await
}
