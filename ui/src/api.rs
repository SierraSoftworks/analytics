//! Thin client over the agent's `/api/v1` REST endpoints.
//!
//! Auth relies on the agent's `HttpOnly` session cookie (sent automatically on
//! same-origin requests); the UI never sees a token. Mutating requests carry a
//! double-submit CSRF token in `X-CSRF-Token`, fetched once and cached. A `401`
//! surfaces as [`ApiError::Unauthorized`] so callers can redirect to login.

use std::cell::RefCell;

use analytics_api::{
    AdminUser, CsrfToken, ExceptionGroup, ExceptionGroupDetail, GlobalException, Instance, Overview,
    Pixel, PixelInput, Project, ProjectInput, Source, SourceInput, Stats, TriageInput,
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
    let resp = Request::get(&format!("{API_BASE}/csrf")).send().await.map_err(net)?;
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

async fn get_json<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
    let resp = Request::get(&format!("{API_BASE}{path}")).send().await.map_err(net)?;
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
        let resp = build(&token).map_err(net)?.send().await.map_err(net)?;
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
    let resp = mutate(|token| {
        Request::post(&url).header(CSRF_HEADER, token).json(body)
    })
    .await?;
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
    let resp = Request::get(&format!("{API_BASE}/me")).send().await.map_err(net)?;
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

pub async fn overview(range: &str) -> Result<Overview, ApiError> {
    get_json(&format!("/overview?{range}")).await
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

pub async fn project_stats(id: &str, range: &str) -> Result<Stats, ApiError> {
    get_json(&format!("/projects/{}/stats?{range}", enc(id))).await
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

pub async fn list_exceptions(project_id: &str, range: &str) -> Result<Vec<ExceptionGroup>, ApiError> {
    get_json(&format!("/projects/{}/exceptions?{range}", enc(project_id))).await
}

/// Exception groups across every project (and unassigned sources).
pub async fn list_all_exceptions(range: &str) -> Result<Vec<GlobalException>, ApiError> {
    get_json(&format!("/exceptions?{range}")).await
}

pub async fn exception_detail(
    group: &str,
    project: &str,
) -> Result<ExceptionGroupDetail, ApiError> {
    get_json(&format!("/exceptions/{}?project={}", enc(group), enc(project))).await
}

pub async fn set_triage(group: &str, input: &TriageInput) -> Result<(), ApiError> {
    patch_empty(&format!("/exceptions/{}", enc(group)), input).await
}
