//! Browser-side helpers for the server-driven OIDC session. Signing in is a
//! full-page navigation to the agent's login endpoint; the browser never handles
//! tokens.

use crate::api;

fn window() -> web_sys::Window {
    web_sys::window().expect("a browser window should be available")
}

fn current_path() -> String {
    let location = window().location();
    let path = location.pathname().unwrap_or_else(|_| "/".to_string());
    let search = location.search().unwrap_or_default();
    let hash = location.hash().unwrap_or_default();
    format!("{path}{search}{hash}")
}

/// Navigate to the agent's login endpoint, preserving the current location.
pub fn begin_login() {
    let encoded: String = js_sys::encode_uri_component(&current_path()).into();
    let url = format!("/api/v1/auth/login?return_to={encoded}");
    let _ = window().location().set_href(&url);
}

/// Clear the server session and return to the app root.
pub async fn logout() {
    let _ = api::logout().await;
    let _ = window().location().set_href("/");
}
