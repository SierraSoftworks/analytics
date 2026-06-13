use yew::prelude::*;

use crate::app::AuthHandle;

#[function_component(Login)]
pub fn login() -> Html {
    let auth = use_context::<AuthHandle>();
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
