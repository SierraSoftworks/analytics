use yew::prelude::*;

use crate::app::AuthHandle;

/// True if the URL carries `?auth_error=oidc_disabled`, set by the server when a
/// sign-in is attempted with no identity provider configured.
fn oidc_disabled() -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .map(|s| s.contains("auth_error=oidc_disabled"))
        .unwrap_or(false)
}

#[function_component(Login)]
pub fn login() -> Html {
    let auth = use_context::<AuthHandle>();

    // Without an identity provider, clicking "Sign in" would loop forever; explain
    // the configuration requirement instead of offering a dead-end button.
    if oidc_disabled() {
        return html! {
            <div class="center-screen">
                <div class="auth-card">
                    <h1>{ "Sign-in unavailable" }</h1>
                    <p>{ "This server has no identity provider configured, so it cannot sign you in." }</p>
                    <p>
                        { "To run without OIDC, set an allow-all admin ACL (" }
                        <code>{ "acl: \"true\"" }</code>
                        { ") so the dashboard is reachable without authentication." }
                    </p>
                </div>
            </div>
        };
    }

    let onclick = {
        let login = auth.as_ref().map(|a| a.login.clone());
        Callback::from(move |_| {
            if let Some(login) = &login {
                login.emit(());
            }
        })
    };
    html! {
        <div class="center-screen">
            <div class="auth-card">
                <h1>{ "Sign in" }</h1>
                <p>{ "Authentication is required to view the dashboard." }</p>
                <button class="btn btn--primary" {onclick}>{ "Sign in with your identity provider" }</button>
            </div>
        </div>
    }
}

#[function_component(NotFound)]
pub fn not_found() -> Html {
    html! {
        <div class="center-screen">
            <div class="auth-card">
                <h1>{ "404" }</h1>
                <p>{ "That page could not be found." }</p>
            </div>
        </div>
    }
}
