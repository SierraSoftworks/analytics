//! Application root: routing, the authentication gate, and the shared auth context.

use analytics_api::AdminUser;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api::{self, ApiError};
use crate::auth;
use crate::components::Layout;
use crate::pages;

#[derive(Clone, Routable, PartialEq)]
pub enum Route {
    #[at("/")]
    Overview,
    #[at("/projects/:id")]
    Project { id: String },
    #[at("/projects/:project/exceptions/:group")]
    Exception { project: String, group: String },
    #[at("/sources")]
    Sources,
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
            <Layout>{ gate(&auth) }</Layout>
        </ContextProvider<AuthHandle>>
    }
}

/// Render the routed content once access is resolved.
fn gate(auth: &AuthHandle) -> Html {
    match &auth.status {
        AuthStatus::Loading => html! { <p class="muted">{ "Loading…" }</p> },
        AuthStatus::NeedsLogin => html! { <pages::Login /> },
        AuthStatus::Error(message) => html! {
            <div class="alert alert--error">
                { format!("Couldn't verify your session: {message}") }
            </div>
        },
        AuthStatus::SignedIn(_) | AuthStatus::Disabled => {
            html! { <Switch<Route> render={switch} /> }
        }
    }
}

fn switch(route: Route) -> Html {
    match route {
        Route::Overview => html! { <pages::Overview /> },
        Route::Project { id } => html! { <pages::Project {id} /> },
        Route::Exception { project, group } => {
            html! { <pages::ExceptionDetail {project} {group} /> }
        }
        Route::Sources => html! { <pages::Sources /> },
        Route::NotFound => html! { <pages::NotFound /> },
    }
}
