use yew::prelude::*;
use yew_router::prelude::*;

use crate::app::{AuthHandle, Route};

#[derive(Properties, PartialEq)]
pub struct LayoutProps {
    #[prop_or_default]
    pub children: Html,
}

#[function_component(Layout)]
pub fn layout(props: &LayoutProps) -> Html {
    let auth = use_context::<AuthHandle>();
    let user = auth.as_ref().and_then(|a| a.user.clone());
    let signout = auth.as_ref().map(|a| a.signout.clone());

    html! {
        <>
            <header class="topbar">
                <div class="topbar__brand">
                    <Link<Route> to={Route::Overview} classes="brand">{ "Analytics" }</Link<Route>>
                    <span class="brand__by">{ "by Sierra Softworks" }</span>
                </div>
                <nav class="topbar__nav">
                    <Link<Route> to={Route::Overview}>{ "Overview" }</Link<Route>>
                    <Link<Route> to={Route::Sources}>{ "Sources" }</Link<Route>>
                </nav>
                <div class="topbar__user">
                    if let Some(user) = user {
                        <span class="muted">{ user.name }</span>
                        if let Some(signout) = signout {
                            <button
                                class="btn btn--ghost"
                                onclick={Callback::from(move |_| signout.emit(()))}
                            >{ "Sign out" }</button>
                        }
                    }
                </div>
            </header>
            <main class="content">{ props.children.clone() }</main>
        </>
    }
}
