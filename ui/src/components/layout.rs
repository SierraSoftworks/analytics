//! The minimal public chrome (brand header) wrapping the signed-out screens:
//! the sign-in page and the 404. The signed-in app uses [`AppShell`] instead.
//!
//! [`AppShell`]: crate::components::AppShell

use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct PublicLayoutProps {
    #[prop_or_default]
    pub children: Html,
}

#[function_component(PublicLayout)]
pub fn public_layout(props: &PublicLayoutProps) -> Html {
    html! {
        <div class="public-shell">
            <div class="public-header">
                <img src="https://cdn.sierrasoftworks.com/logos/icon.svg" alt="Sierra Softworks" />
                <span>{ "Analytics" }</span>
            </div>
            { props.children.clone() }
        </div>
    }
}
