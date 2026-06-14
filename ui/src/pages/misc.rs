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
            <div class="centered">
                <h1>{ "Sign-in unavailable" }</h1>
                <p class="muted">
                    { "This server has no identity provider configured, so it cannot sign you in." }
                </p>
                <p class="muted">
                    { "To run without OIDC, set an allow-expression admin ACL (e.g. " }
                    <code>{ "acl: \"true\"" }</code>
                    { ") so the dashboard is reachable without authentication." }
                </p>
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
        <div class="centered">
            <h1>{ "Sign in" }</h1>
            <p class="muted">{ "Authentication is required to view the dashboard." }</p>
            <button class="btn btn--primary" {onclick}>{ "Sign in" }</button>
        </div>
    }
}

#[function_component(NotFound)]
pub fn not_found() -> Html {
    html! {
        <div class="centered">
            <h1>{ "404" }</h1>
            <p class="muted">{ "That page could not be found." }</p>
        </div>
    }
}
