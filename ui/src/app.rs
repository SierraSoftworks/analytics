//! Application root: routing, the authentication gate, and the shared auth context.

use analytics_api::AdminUser;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api::{self, ApiError};
use crate::auth;
use crate::components::{AppShell, PublicLayout};
use crate::pages;

#[derive(Clone, Routable, PartialEq)]
pub enum Route {
    #[at("/")]
    Overview,
    #[at("/exceptions")]
    Exceptions,
    #[at("/projects/:id")]
    Project { id: String },
    #[at("/projects/:project/exceptions/:group")]
    Exception { project: String, group: String },
    #[at("/pixels")]
    Pixels,
    #[at("/settings")]
    Settings,
    #[not_found]
    #[at("/404")]
    NotFound,
}

#[derive(Clone, PartialEq)]
pub enum AuthStatus {
    Loading,
    /// OIDC is disabled; the API is reachable without signing in.
    Disabled,
    SignedIn(AdminUser),
    NeedsLogin,
    Error(String),
}

#[derive(Clone, PartialEq)]
pub struct AuthHandle {
    pub status: AuthStatus,
    pub user: Option<AdminUser>,
    pub login: Callback<()>,
    pub signout: Callback<()>,
    /// Drop back to the sign-in screen, e.g. when a request 401s mid-session.
    pub relogin: Callback<()>,
}

#[hook]
fn use_auth() -> AuthHandle {
    let status = use_state(|| AuthStatus::Loading);
    {
        let status = status.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                match api::me().await {
                    Ok(Some(user)) => status.set(AuthStatus::SignedIn(user)),
                    Ok(None) => status.set(AuthStatus::Disabled),
                    Err(ApiError::Unauthorized) => status.set(AuthStatus::NeedsLogin),
                    Err(e) => status.set(AuthStatus::Error(e.to_string())),
                }
            });
            || ()
        });
    }

    let login = Callback::from(|_| auth::begin_login());
    let signout = Callback::from(|_| {
        spawn_local(async {
            auth::logout().await;
        })
    });
    let relogin = {
        let status = status.clone();
        Callback::from(move |_| status.set(AuthStatus::NeedsLogin))
    };
    let user = match &*status {
        AuthStatus::SignedIn(user) => Some(user.clone()),
        _ => None,
    };

    AuthHandle {
        status: (*status).clone(),
        user,
        login,
        signout,
        relogin,
    }
}

#[function_component(App)]
pub fn app() -> Html {
    html! {
        <BrowserRouter>
            <AppInner />
        </BrowserRouter>
    }
}

#[function_component(AppInner)]
fn app_inner() -> Html {
    let auth = use_auth();
    html! {
        <ContextProvider<AuthHandle> context={auth.clone()}>
            { gate(&auth) }
        </ContextProvider<AuthHandle>>
    }
}

/// Render the routed app once access is resolved, falling back to the public
/// chrome (sign-in / status) while it is not.
fn gate(auth: &AuthHandle) -> Html {
    match &auth.status {
        AuthStatus::Loading => html! {
            <PublicLayout>
                <div class="center-screen"><p class="muted">{ "Loading…" }</p></div>
            </PublicLayout>
        },
        AuthStatus::NeedsLogin => html! { <PublicLayout><pages::Login /></PublicLayout> },
        AuthStatus::Error(message) => html! {
            <PublicLayout>
                <div class="center-screen">
                    <div class="auth-card">
                        <h1>{ "Couldn't verify your session" }</h1>
                        <p>{ message.clone() }</p>
                    </div>
                </div>
            </PublicLayout>
        },
        AuthStatus::SignedIn(_) | AuthStatus::Disabled => html! {
            <AppShell><Switch<Route> render={switch} /></AppShell>
        },
    }
}

fn switch(route: Route) -> Html {
    match route {
        Route::Overview => html! { <pages::Overview /> },
        Route::Exceptions => html! { <pages::Exceptions /> },
        Route::Project { id } => html! { <pages::Project {id} /> },
        Route::Exception { project, group } => {
            html! { <pages::ExceptionDetail {project} {group} /> }
        }
        Route::Pixels => html! { <pages::Pixels /> },
        Route::Settings => html! { <pages::Settings /> },
        Route::NotFound => html! { <pages::NotFound /> },
    }
}
