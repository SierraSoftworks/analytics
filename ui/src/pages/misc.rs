use yew::prelude::*;
use yewi::prelude::{Button, ButtonVariant, EmptyState, InlineCode, NavTarget, icons};

use crate::app::{AuthHandle, Route};

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
            <EmptyState title="Sign-in unavailable" icon={icons::LOCK}>
                <p>{ "This server has no identity provider configured, so it cannot sign you in." }</p>
                <p>
                    { "To run without OIDC, set an allow-all admin ACL (" }
                    <InlineCode>{ "acl: \"true\"" }</InlineCode>
                    { ") so the dashboard is reachable without authentication." }
                </p>
            </EmptyState>
        };
    }

    let on_click = {
        let login = auth.as_ref().map(|a| a.login.clone());
        Callback::from(move |_: MouseEvent| {
            if let Some(login) = &login {
                login.emit(());
            }
        })
    };
    let actions = html! {
        <Button variant={ButtonVariant::Primary} {on_click}>{ "Sign in with your identity provider" }</Button>
    };
    html! {
        <EmptyState title="Sign in" icon={icons::LOCK} {actions}>
            { "Authentication is required to view the dashboard." }
        </EmptyState>
    }
}

#[function_component(NotFound)]
pub fn not_found() -> Html {
    let actions = html! { <Button href={NavTarget::route(Route::Overview)}>{ "Back to the dashboard" }</Button> };
    html! {
        <EmptyState title="Page not found" icon={icons::SEARCH_X} {actions}>
            { "That page could not be found." }
        </EmptyState>
    }
}
