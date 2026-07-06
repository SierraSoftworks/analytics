//! OpenID Connect machinery for the server-driven admin login: discovery + JWKS
//! fetching/caching, ID token validation, the authorization-URL builder, the
//! confidential token exchange, the refresh-token grant, and claim filtering.
//!
//! The agent performs the entire Authorization Code + PKCE exchange itself; the
//! issued ID token is stored in an `HttpOnly` session cookie and never exposed to
//! JavaScript. The discovery document and signing keys are cached for an hour.
//! Ported from SierraSoftworks/automate, adapted to this service's `AppState`.

use std::borrow::Cow;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use actix_web::http::header::HeaderMap;
use base64::Engine;
use filt_rs::{FilterValue, Filterable};
use sha2::{Digest, Sha256};

use crate::config::OidcConfig;
use crate::errors::{Result, ResultExt};

const ADVICE_PROVIDER: &[&str] = &[
    "Ensure that `web.admin.oidc.endpoint` points at a valid OIDC provider.",
    "Check that the provider is reachable from this server.",
];

const ADVICE_REAUTH: &[&str] = &["Sign in again to obtain a fresh session."];

/// JWT claims carrying protocol semantics, hidden from the ACL filter.
const EXCLUDED_CLAIMS: &[&str] = &[
    "exp",
    "nbf",
    "iat",
    "iss",
    "aud",
    "jti",
    "nonce",
    "at_hash",
    "c_hash",
    "azp",
    "auth_time",
];

const CACHE_TTL: Duration = Duration::from_secs(60 * 60);

/// Single-provider TTL cache for the discovery document and JWKS.
#[derive(Default)]
pub struct OidcCache {
    discovery: Mutex<Option<(Instant, OidcDiscovery)>>,
    jwks: Mutex<Option<(Instant, jsonwebtoken::jwk::JwkSet)>>,
}

impl OidcCache {
    fn discovery(&self) -> Option<OidcDiscovery> {
        let guard = self.discovery.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .filter(|(at, _)| at.elapsed() < CACHE_TTL)
            .map(|(_, v)| v.clone())
    }

    fn store_discovery(&self, value: OidcDiscovery) {
        *self.discovery.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), value));
    }

    fn jwks(&self) -> Option<jsonwebtoken::jwk::JwkSet> {
        let guard = self.jwks.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .filter(|(at, _)| at.elapsed() < CACHE_TTL)
            .map(|(_, v)| v.clone())
    }

    fn store_jwks(&self, value: jsonwebtoken::jwk::JwkSet) {
        *self.jwks.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), value));
    }
}

/// The subset of the OIDC discovery document we rely upon.
#[derive(Clone, serde::Deserialize)]
pub struct OidcDiscovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

#[derive(serde::Deserialize)]
struct ProviderTokenResponse {
    id_token: String,
    refresh_token: Option<String>,
}

/// Tokens issued by the provider's token endpoint. `id_token` becomes the session
/// cookie; `refresh_token` (when the provider issues one) lets the agent renew the
/// session without another interactive login.
pub struct TokenSet {
    pub id_token: String,
    pub refresh_token: Option<String>,
}

/// A PKCE verifier and its derived S256 challenge.
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

fn base64url(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// Generate a high-entropy PKCE verifier (256 bits) and its S256 challenge.
pub fn generate_pkce() -> PkcePair {
    let verifier = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let challenge = base64url(Sha256::digest(verifier.as_bytes()).as_slice());
    PkcePair {
        verifier,
        challenge,
    }
}

/// Generate an opaque random token for use as OAuth `state` or a CSRF token.
pub fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Build the provider's authorization URL for Authorization Code + PKCE.
pub fn authorize_url(
    oidc: &OidcConfig,
    discovery: &OidcDiscovery,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> Result<String> {
    let mut scopes = vec!["openid".to_string()];
    for scope in &oidc.scopes {
        if scope != "openid" {
            scopes.push(scope.clone());
        }
    }
    let scope = scopes.join(" ");

    let mut url = reqwest::Url::parse(&discovery.authorization_endpoint).wrap_system_err(
        "The OIDC provider advertised an invalid authorization endpoint.",
        ADVICE_PROVIDER,
    )?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &oidc.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", &scope)
        .append_pair("state", state)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url.to_string())
}

/// A [`Filterable`] view over an admin request for ACL evaluation.
pub struct AdminRequestFilter<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub client_ip: Option<String>,
    pub headers: &'a HeaderMap,
    pub claims: Option<&'a serde_json::Map<String, serde_json::Value>>,
}

impl Filterable for AdminRequestFilter<'_> {
    fn get(&self, key: &str) -> FilterValue<'_> {
        match key {
            "method" => self.method.into(),
            "path" => self.path.into(),
            "client_ip" => self.client_ip.clone().into(),
            key if key.starts_with("headers.") => {
                let name = &key["headers.".len()..];
                match self.headers.get(name).and_then(|v| v.to_str().ok()) {
                    Some(value) => value.to_string().into(),
                    None => FilterValue::Null,
                }
            }
            key if key.starts_with("claims.") => {
                let name = &key["claims.".len()..];
                match self.claims.and_then(|c| c.get(name)) {
                    Some(value) => json_to_filter_value(value),
                    None => FilterValue::Null,
                }
            }
            _ => FilterValue::Null,
        }
    }
}

/// Convert a JSON claim value into a [`FilterValue`] (filt-rs has no built-in
/// conversion). Objects, which the ACL language can't express, become `Null`.
fn json_to_filter_value(value: &serde_json::Value) -> FilterValue<'static> {
    match value {
        serde_json::Value::Null => FilterValue::Null,
        serde_json::Value::Bool(b) => FilterValue::Bool(*b),
        serde_json::Value::Number(n) => n
            .as_f64()
            .map(FilterValue::Number)
            .unwrap_or(FilterValue::Null),
        serde_json::Value::String(s) => FilterValue::String(Cow::Owned(s.clone())),
        serde_json::Value::Array(items) => {
            FilterValue::Tuple(items.iter().map(json_to_filter_value).collect())
        }
        serde_json::Value::Object(_) => FilterValue::Null,
    }
}

/// Fetch (and cache) the OIDC discovery document.
pub async fn discovery(
    http: &reqwest::Client,
    cache: &OidcCache,
    oidc: &OidcConfig,
) -> Result<OidcDiscovery> {
    if let Some(cached) = cache.discovery() {
        return Ok(cached);
    }
    let url = format!(
        "{}/.well-known/openid-configuration",
        oidc.endpoint.trim_end_matches('/')
    );
    let document: OidcDiscovery = http
        .get(&url)
        .send()
        .await
        .wrap_system_err(
            "Failed to fetch the OIDC discovery document.",
            ADVICE_PROVIDER,
        )?
        .error_for_status()
        .wrap_system_err(
            "The OIDC provider returned an error for its discovery document.",
            ADVICE_PROVIDER,
        )?
        .json()
        .await
        .wrap_system_err(
            "Failed to parse the OIDC discovery document.",
            ADVICE_PROVIDER,
        )?;
    cache.store_discovery(document.clone());
    Ok(document)
}

/// Fetch the JSON Web Key Set from the provider (no caching).
async fn fetch_jwks(
    http: &reqwest::Client,
    discovery: &OidcDiscovery,
) -> Result<jsonwebtoken::jwk::JwkSet> {
    http.get(&discovery.jwks_uri)
        .send()
        .await
        .wrap_system_err(
            "Failed to fetch the OIDC signing keys (JWKS).",
            ADVICE_PROVIDER,
        )?
        .error_for_status()
        .wrap_system_err(
            "The OIDC provider returned an error for its signing keys.",
            ADVICE_PROVIDER,
        )?
        .json()
        .await
        .wrap_system_err("Failed to parse the OIDC signing keys.", ADVICE_PROVIDER)
}

/// Fetch (and cache) the JSON Web Key Set used to verify token signatures.
async fn jwks(
    http: &reqwest::Client,
    cache: &OidcCache,
    discovery: &OidcDiscovery,
) -> Result<jsonwebtoken::jwk::JwkSet> {
    if let Some(cached) = cache.jwks() {
        return Ok(cached);
    }
    let keys = fetch_jwks(http, discovery).await?;
    cache.store_jwks(keys.clone());
    Ok(keys)
}

/// The `kid` (key id) from a token header, if present.
fn token_kid(token: &str) -> Option<String> {
    jsonwebtoken::decode_header(token).ok().and_then(|h| h.kid)
}

/// Validate an ID token's signature and registered claims, returning the claims.
pub async fn validate_token(
    http: &reqwest::Client,
    cache: &OidcCache,
    oidc: &OidcConfig,
    token: &str,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let discovery = discovery(http, cache, oidc).await?;
    let mut key_set = jwks(http, cache, &discovery).await?;

    // If the token's signing key isn't in the cached set, the provider may have
    // rotated keys; refetch once (bypassing the cache) before rejecting it.
    if let Some(kid) = token_kid(token)
        && key_set.find(&kid).is_none()
    {
        key_set = fetch_jwks(http, &discovery).await?;
        cache.store_jwks(key_set.clone());
    }

    verify_token(&oidc.client_id, &discovery.issuer, &key_set, token)
}

/// Pure verification core (no fetching), exercised directly by tests.
fn verify_token(
    client_id: &str,
    issuer: &str,
    key_set: &jsonwebtoken::jwk::JwkSet,
    token: &str,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let header = jsonwebtoken::decode_header(token).wrap_user_err(
        "The admin session token could not be decoded.",
        ADVICE_REAUTH,
    )?;

    // Reject symmetric algorithms to prevent algorithm-confusion attacks against
    // the asymmetric keys published via JWKS.
    if matches!(
        header.alg,
        jsonwebtoken::Algorithm::HS256
            | jsonwebtoken::Algorithm::HS384
            | jsonwebtoken::Algorithm::HS512
    ) {
        return Err(human_errors::user(
            "The admin session token is signed with an unsupported algorithm.",
            &["The OIDC provider must sign ID tokens with an asymmetric algorithm (e.g. RS256)."],
        ));
    }

    let kid = header.kid.ok_or_else(|| {
        human_errors::user(
            "The admin session token does not identify a signing key.",
            ADVICE_REAUTH,
        )
    })?;
    let jwk = key_set.find(&kid).ok_or_else(|| {
        human_errors::user(
            "The admin session token was signed with an unknown key.",
            ADVICE_REAUTH,
        )
    })?;
    let decoding_key = jsonwebtoken::DecodingKey::from_jwk(jwk).wrap_system_err(
        "Failed to construct a verification key from the provider's JWKS.",
        ADVICE_PROVIDER,
    )?;

    let mut validation = jsonwebtoken::Validation::new(header.alg);
    validation.set_audience(&[client_id]);
    validation.set_issuer(&[issuer]);
    validation.validate_exp = true;
    validation.validate_nbf = true;

    let data = jsonwebtoken::decode::<serde_json::Map<String, serde_json::Value>>(
        token,
        &decoding_key,
        &validation,
    )
    .wrap_user_err("The admin session token failed validation.", ADVICE_REAUTH)?;
    Ok(data.claims)
}

/// Exchange an authorization code (+ PKCE verifier) for the issued tokens. The
/// confidential client secret never leaves the server.
pub async fn exchange_code(
    http: &reqwest::Client,
    oidc: &OidcConfig,
    discovery: &OidcDiscovery,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<TokenSet> {
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", code_verifier),
        ("redirect_uri", redirect_uri),
        ("client_id", oidc.client_id.as_str()),
        ("client_secret", oidc.client_secret.as_str()),
    ];
    token_request(
        http,
        &discovery.token_endpoint,
        &params,
        "The OIDC provider rejected the authorization code exchange.",
        &["Start the sign-in process again from the beginning."],
    )
    .await
}

/// Renew a session from a previously issued refresh token, returning a fresh ID
/// token (and a rotated refresh token when the provider supplies one). Providers
/// that don't rotate refresh tokens omit it from the response, so the caller's
/// token is carried over to keep the session renewable.
pub async fn refresh_tokens(
    http: &reqwest::Client,
    oidc: &OidcConfig,
    discovery: &OidcDiscovery,
    refresh_token: &str,
) -> Result<TokenSet> {
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", oidc.client_id.as_str()),
        ("client_secret", oidc.client_secret.as_str()),
    ];
    let mut tokens = token_request(
        http,
        &discovery.token_endpoint,
        &params,
        "The OIDC provider rejected the session renewal.",
        ADVICE_REAUTH,
    )
    .await?;
    if tokens.refresh_token.is_none() {
        tokens.refresh_token = Some(refresh_token.to_string());
    }
    Ok(tokens)
}

/// POST a form-encoded grant to the provider's token endpoint and parse the
/// issued tokens.
async fn token_request(
    http: &reqwest::Client,
    token_endpoint: &str,
    params: &[(&str, &str)],
    rejection: &'static str,
    rejection_advice: &'static [&'static str],
) -> Result<TokenSet> {
    let response: ProviderTokenResponse = http
        .post(token_endpoint)
        .form(params)
        .send()
        .await
        .wrap_system_err(
            "Failed to reach the OIDC provider's token endpoint.",
            ADVICE_PROVIDER,
        )?
        .error_for_status()
        .wrap_user_err(rejection, rejection_advice)?
        .json()
        .await
        .wrap_system_err(
            "Failed to parse the token response from the provider.",
            ADVICE_PROVIDER,
        )?;
    Ok(TokenSet {
        id_token: response.id_token,
        refresh_token: response.refresh_token,
    })
}

/// Derive the display identity from a validated claim set.
pub fn admin_user_from_claims(
    claims: &serde_json::Map<String, serde_json::Value>,
) -> analytics_api::AdminUser {
    let str_claim = |key: &str| {
        claims
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.is_empty())
    };
    let email = str_claim("email");
    let name = str_claim("name")
        .or_else(|| str_claim("preferred_username"))
        .or_else(|| email.clone())
        .or_else(|| str_claim("sub"))
        .unwrap_or_else(|| "Signed in".to_string());
    analytics_api::AdminUser { name, email }
}

/// Strip registered/temporal claims so the ACL only sees user-meaningful ones.
pub fn filterable_claims(
    claims: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    claims
        .iter()
        .filter(|(k, _)| !EXCLUDED_CLAIMS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hs256_token() -> String {
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        let claims = serde_json::json!({ "sub": "u", "aud": "client", "iss": "https://idp" });
        jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(b"secret"),
        )
        .unwrap()
    }

    #[test]
    fn rejects_symmetric_algorithms() {
        let keys = jsonwebtoken::jwk::JwkSet { keys: vec![] };
        assert!(verify_token("client", "https://idp", &keys, &hs256_token()).is_err());
    }

    #[test]
    fn rejects_malformed_tokens() {
        let keys = jsonwebtoken::jwk::JwkSet { keys: vec![] };
        assert!(verify_token("client", "https://idp", &keys, "not-a-jwt").is_err());
    }

    #[test]
    fn filterable_claims_strips_registered() {
        let mut claims = serde_json::Map::new();
        claims.insert("sub".into(), serde_json::json!("u"));
        claims.insert("groups".into(), serde_json::json!(["admins"]));
        claims.insert("exp".into(), serde_json::json!(1));
        claims.insert("iss".into(), serde_json::json!("x"));
        let filtered = filterable_claims(&claims);
        assert!(filtered.contains_key("sub"));
        assert!(filtered.contains_key("groups"));
        assert!(!filtered.contains_key("exp"));
        assert!(!filtered.contains_key("iss"));
    }

    /// Build a discovery document pointing every endpoint at the given mock server.
    fn mock_discovery(base: &str) -> OidcDiscovery {
        OidcDiscovery {
            issuer: base.to_string(),
            authorization_endpoint: format!("{base}/authorize"),
            token_endpoint: format!("{base}/token"),
            jwks_uri: format!("{base}/jwks"),
        }
    }

    fn test_oidc_config(base: &str) -> crate::config::OidcConfig {
        crate::config::OidcConfig {
            endpoint: base.to_string(),
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            scopes: vec![],
        }
    }

    #[tokio::test]
    async fn exchange_code_captures_the_refresh_token() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=authorization_code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id_token": "header.payload.sig",
                "refresh_token": "refresh-123",
            })))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let tokens = exchange_code(
            &http,
            &test_oidc_config(&server.uri()),
            &mock_discovery(&server.uri()),
            "auth-code",
            "verifier",
            "http://localhost/api/v1/auth/callback",
        )
        .await
        .unwrap();
        assert_eq!(tokens.id_token, "header.payload.sig");
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-123"));
    }

    #[tokio::test]
    async fn refresh_reuses_the_old_token_when_the_provider_does_not_rotate() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id_token": "renewed.id.token",
            })))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let tokens = refresh_tokens(
            &http,
            &test_oidc_config(&server.uri()),
            &mock_discovery(&server.uri()),
            "refresh-123",
        )
        .await
        .unwrap();
        assert_eq!(tokens.id_token, "renewed.id.token");
        assert_eq!(
            tokens.refresh_token.as_deref(),
            Some("refresh-123"),
            "a non-rotating provider's response must not discard the caller's refresh token"
        );
    }

    #[tokio::test]
    async fn refresh_adopts_a_rotated_token_and_surfaces_rejections() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=live-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id_token": "renewed.id.token",
                "refresh_token": "rotated-456",
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=revoked-token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
            })))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let oidc = test_oidc_config(&server.uri());
        let discovery = mock_discovery(&server.uri());

        let tokens = refresh_tokens(&http, &oidc, &discovery, "live-token")
            .await
            .unwrap();
        assert_eq!(tokens.refresh_token.as_deref(), Some("rotated-456"));

        assert!(
            refresh_tokens(&http, &oidc, &discovery, "revoked-token")
                .await
                .is_err(),
            "a rejected grant must surface as an error so the session is dropped"
        );
    }

    #[test]
    fn admin_request_filter_exposes_claims_to_acl() {
        let mut claims = serde_json::Map::new();
        claims.insert("groups".into(), serde_json::json!(["admins", "users"]));
        let headers = HeaderMap::new();
        let filter = AdminRequestFilter {
            method: "GET",
            path: "/api/v1/me",
            client_ip: Some("127.0.0.1".into()),
            headers: &headers,
            claims: Some(&claims),
        };
        let acl = filt_rs::Filter::new("\"admins\" in claims.groups").unwrap();
        assert!(acl.matches(&filter).unwrap());
        let deny = filt_rs::Filter::new("\"superadmin\" in claims.groups").unwrap();
        assert!(!deny.matches(&filter).unwrap());
    }
}
