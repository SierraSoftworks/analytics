use actix_http::body::BoxBody;
use actix_web::{HttpResponse, error, http::StatusCode};
use std::fmt;
use tracing_batteries::prelude::*;

#[derive(Debug, Serialize, Deserialize)]
pub struct APIError {
    pub code: u16,
    pub error: String,
    pub message: String,
}

impl APIError {
    pub fn new(code: u16, error: &str, message: &str) -> Self {
        Self {
            code,
            error: error.to_string(),
            message: message.to_string(),
        }
    }
}

impl error::ResponseError for APIError {
    fn error_response(&self) -> HttpResponse<BoxBody> {
        HttpResponse::build(self.status_code())
            .content_type("application/json; charset=utf-8")
            .json(self)
    }

    fn status_code(&self) -> StatusCode {
        StatusCode::from_u16(self.code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

impl fmt::Display for APIError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[HTTP {} {}] {}", self.code, self.error, self.message)
    }
}

impl From<azure_core::error::Error> for APIError {
    fn from(err: azure_core::error::Error) -> Self {
        error!({ exception.message = %err }, "We were unable to call Azure Table Storage");

        sentry::capture_error(&err);

        Self::new(
            500,
            "Internal Server Error",
            "We ran into a problem, this has been reported and will be looked at.",
        )
    }
}

impl From<actix::MailboxError> for APIError {
    fn from(err: actix::MailboxError) -> Self {
        error!({ exception.message = %err }, "We were unable to call Azure Table Storage");

        sentry::capture_error(&err);

        Self::new(
            500,
            "Internal Server Error",
            "We ran into a problem, this has been reported and will be looked at.",
        )
    }
}
