use std::{
    pin::Pin,
    task::{Context, Poll},
};

use actix_service::*;
use actix_web::dev::*;
use actix_web::{Error, http::header::HeaderMap};
use futures::{
    Future, FutureExt,
    future::{Ready, ok},
};
use opentelemetry::propagation::Extractor;
use tracing_batteries::prelude::*;

pub struct TracingLogger;

impl<S, B> Transform<S, ServiceRequest> for TracingLogger
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = TracingLoggerMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ok(TracingLoggerMiddleware { service })
    }
}

#[doc(hidden)]
pub struct TracingLoggerMiddleware<S> {
    service: S,
}

impl<S, B> Service<ServiceRequest> for TracingLoggerMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let user_agent = req
            .headers()
            .get("User-Agent")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");

        // Never log client IPs: redact IP-bearing (and auth/cookie) headers.
        let headers = format_headers(req.headers());

        let span = info_span!(
            "request",
            "otel.kind" = "server",
            "otel.name" = req.match_pattern().unwrap_or_else(|| req.uri().path().to_string()),
            "net.transport" = "IP.TCP",
            "http.target" = %req.uri(),
            "http.user_agent" = %user_agent,
            "http.status_code" = EmptyField,
            "http.method" = %req.method(),
            "http.url" = %req.match_pattern().unwrap_or_else(|| req.path().into()),
            "http.headers" = %headers,
        );

        // Propagate OpenTelemetry parent span context information
        let context = opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderMapExtractor::from(req.headers()))
        });

        span.set_parent(context);

        let fut = self
            .service
            .call(req)
            .map(move |outcome| match &outcome {
                Ok(response) => {
                    Span::current()
                        .record("http.status_code", display(response.response().status()));
                    outcome
                }
                Err(error) => {
                    Span::current().record(
                        "http.status_code",
                        display(error.as_response_error().status_code()),
                    );
                    outcome
                }
            })
            .instrument(span);

        Box::pin(fut)
    }
}

/// Headers whose values can reveal a client IP, or are otherwise sensitive, are
/// redacted before being attached to a trace — the service never logs IPs.
const REDACTED_HEADERS: &[&str] = &[
    "x-forwarded-for",
    "x-real-ip",
    "forwarded",
    "cf-connecting-ip",
    "true-client-ip",
    "fastly-client-ip",
    "x-client-ip",
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
];

fn format_headers(headers: &HeaderMap) -> String {
    headers
        .iter()
        .map(|(name, value)| {
            if REDACTED_HEADERS.contains(&name.as_str()) {
                format!("{name}: [redacted]")
            } else {
                format!("{name}: {value:?}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct HeaderMapExtractor<'a> {
    headers: &'a HeaderMap,
}

impl<'a> From<&'a HeaderMap> for HeaderMapExtractor<'a> {
    fn from(headers: &'a HeaderMap) -> Self {
        HeaderMapExtractor { headers }
    }
}

impl<'a> Extractor for HeaderMapExtractor<'a> {
    fn get(&self, key: &str) -> Option<&'a str> {
        self.headers.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.headers.keys().map(|v| v.as_str()).collect()
    }
}
