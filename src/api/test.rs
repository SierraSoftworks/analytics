use crate::api::configure;
use crate::models::*;
use actix_web::{
    App,
    test::{self, read_body_json},
};
use chrono::{Duration, Utc};
use serde::de::DeserializeOwned;
use tracing_batteries::prelude::*;

pub fn test_log_init() {
    let _ = env_logger::builder()
        .is_test(true)
        .filter_level(log::LevelFilter::Debug)
        .try_init();
}

pub async fn get_test_app(
    state: GlobalState,
) -> impl actix_web::dev::Service<
    actix_http::Request,
    Response = actix_web::dev::ServiceResponse,
    Error = actix_web::Error,
> {
    test::init_service(
        App::new()
            .app_data(actix_web::web::Data::new(state.clone()))
            .configure(configure),
    )
    .await
}

pub fn assert_location_header(header: &actix_web::http::header::HeaderMap, prefix: &str) {
    let location = header
        .get("Location")
        .expect("a location header")
        .to_str()
        .expect("a non-empty location header");

    debug!("Got location header: {}", location);

    assert!(location.contains(prefix));

    let id =
        String::from(&location[location.find(prefix).expect("index of path") + prefix.len()..]);
    assert_ne!(id, "");
}

pub async fn assert_status(
    resp: actix_web::dev::ServiceResponse,
    expected_status: http::StatusCode,
) -> actix_web::dev::ServiceResponse {
    if expected_status != resp.status() {
        let status = resp.status();
        if resp
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .map(|h| h.starts_with("application/json"))
            .unwrap_or_default()
        {
            let err: super::APIError = get_content(resp).await;
            panic!(
                "Unexpected response code (got == expected)\n  got: {}\n  expected: {}\n  error: {}",
                status, expected_status, err
            )
        } else {
            panic!(
                "Unexpected response code (got == expected)\n  got: {}\n  expected: {}\n  error: {}",
                status, expected_status, "No error message"
            )
        }
    } else {
        resp
    }
}

pub async fn get_content<T: DeserializeOwned>(resp: actix_web::dev::ServiceResponse) -> T {
    read_body_json(resp).await
}
