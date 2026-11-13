//! The minimal public chrome (brand bar, no navigation) wrapping the
//! signed-out screens: loading, sign-in, and session errors. The signed-in app
//! uses [`AppShell`] instead.
//!
//! [`AppShell`]: crate::components::AppShell

use yew::prelude::*;
use yewi::prelude::{AppBar, AppShell as Shell, ThemeToggle};

use crate::components::shell::{brand, footer};

#[derive(Properties, PartialEq)]
pub struct PublicLayoutProps {
    #[prop_or_default]
    pub children: Html,
}

#[function_component(PublicLayout)]
pub fn public_layout(props: &PublicLayoutProps) -> Html {
    let bar = html! { <AppBar brand={brand()} actions={html! { <ThemeToggle /> }} /> };
    html! {
        <Shell {bar} class="app-shell app-shell--bare">
            <div class="center-screen">{ props.children.clone() }</div>
            { footer() }
        </Shell>
    }
}
