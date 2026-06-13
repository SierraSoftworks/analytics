//! Request helpers. The client IP obtained here is used **only** as a transient
//! rate-limit key and is never logged or stored.

use actix_web::HttpRequest;

/// The client IP, honouring `X-Forwarded-For`/`X-Real-IP` only when the operator
/// has opted into trusting a reverse proxy. For rate limiting only.
pub fn client_ip(req: &HttpRequest, trust_proxy: bool) -> String {
    if trust_proxy {
        if let Some(forwarded) = header(req, "x-forwarded-for") {
            if let Some(first) = forwarded.split(',').next() {
                let ip = first.trim();
                if !ip.is_empty() {
                    return ip.to_string();
                }
            }
        }
        if let Some(real) = header(req, "x-real-ip") {
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

/// Whether the request asserts Do-Not-Track or Global Privacy Control.
pub fn privacy_signal(req: &HttpRequest) -> bool {
    header(req, "dnt").as_deref() == Some("1") || header(req, "sec-gpc").as_deref() == Some("1")
}

/// Read a header as a string, if present and valid UTF-8.
pub fn header(req: &HttpRequest, name: &str) -> Option<String> {
    req.headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Whether the original request reached us over HTTPS. `X-Forwarded-Proto` is only
/// consulted when the deployment is configured to trust its proxy.
pub fn is_https(trust_proxy: bool, req: &HttpRequest) -> bool {
    if trust_proxy {
        if let Some(proto) = header(req, "x-forwarded-proto") {
            return proto
                .split(',')
                .next()
                .map(|p| p.trim().eq_ignore_ascii_case("https"))
                .unwrap_or(false);
        }
    }
    req.uri().scheme_str() == Some("https")
}

/// The externally visible base URL, preferring the configured `web.base_url` and
/// otherwise reconstructing it from the request host + scheme. Forwarding headers
/// are only trusted when `web.trust_proxy` is enabled.
pub fn base_url(config: &crate::config::WebConfig, req: &HttpRequest) -> Option<String> {
    if let Some(base) = &config.base_url {
        return Some(base.trim_end_matches('/').to_string());
    }
    let host = if config.trust_proxy {
        header(req, "x-forwarded-host").or_else(|| header(req, "host"))
    } else {
        header(req, "host")
    }?;
    let scheme = if is_https(config.trust_proxy, req) {
        "https"
    } else {
        "http"
    };
    Some(format!("{scheme}://{host}"))
}
