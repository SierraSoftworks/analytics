use serde::{Deserialize, Serialize};

/// The identity of an authenticated administrator, derived from OIDC claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminUser {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// A CSRF token issued to the frontend for the double-submit protection scheme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CsrfToken {
    pub token: String,
}
