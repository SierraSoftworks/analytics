//! The persistent top app bar: the mobile navigation toggle, brand mark, the
//! filter **query bar** (a filt-rs expression editing the same state the chips
//! render), and the signed-in user chip.

use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::app::{AuthHandle, AuthStatus, Route};
use crate::components::icons;
use crate::filters::{Query, use_apply_filters, use_filters, validate_expression};

/// Up-to-two uppercase initials from a display name or email.
fn initials(name: &str) -> String {
    let from_words: String = name
        .split(|c: char| c.is_whitespace() || c == '.' || c == '@' || c == '_' || c == '-')
        .filter(|w| !w.is_empty())
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect();
    let initials = if from_words.is_empty() {
        name.chars().take(2).collect()
    } else {
        from_words
    };
    initials.to_uppercase()
}

/// The query bar: shows the active filter expression, and applies an edited
/// one on Enter (Escape reverts). Invalid syntax is flagged inline and never
/// navigates. Chips and click-to-filter edit the same expression — this is
/// just the power-user view of it.
#[function_component(QueryBar)]
fn query_bar() -> Html {
    let filters = use_filters();
    let apply = use_apply_filters();
    let live = filters.query.to_expression();

    let draft = use_state(String::new);
    let editing = use_state(|| false);
    let error = use_state(|| None::<String>);
    let input_ref = use_node_ref();

    let value = if *editing {
        (*draft).clone()
    } else {
        live.clone()
    };

    let onfocus = {
        let (draft, editing, live) = (draft.clone(), editing.clone(), live.clone());
        Callback::from(move |_: FocusEvent| {
            if !*editing {
                draft.set(live.clone());
                editing.set(true);
            }
        })
    };
    let oninput = {
        let (draft, error) = (draft.clone(), error.clone());
        Callback::from(move |e: InputEvent| {
            error.set(None);
            draft.set(e.target_unchecked_into::<HtmlInputElement>().value());
        })
    };
    let onkeydown = {
        let (draft, editing, error) = (draft.clone(), editing.clone(), error.clone());
        let (apply, filters, input_ref) = (apply.clone(), filters.clone(), input_ref.clone());
        Callback::from(move |e: KeyboardEvent| match e.key().as_str() {
            "Enter" => {
                e.prevent_default();
                let expression = draft.trim().to_string();
                if let Some(message) = validate_expression(&expression) {
                    error.set(Some(message));
                    return;
                }
                apply.emit(filters.with_query(Query::parse(&expression)));
                editing.set(false);
                error.set(None);
                if let Some(input) = input_ref.cast::<HtmlInputElement>() {
                    let _ = input.blur();
                }
            }
            "Escape" => {
                e.prevent_default();
                editing.set(false);
                error.set(None);
                if let Some(input) = input_ref.cast::<HtmlInputElement>() {
                    let _ = input.blur();
                }
            }
            _ => {}
        })
    };

    html! {
        <div class={classes!("query-bar", error.is_some().then_some("query-bar--invalid"))}>
            <span class="query-bar__icon" aria-hidden="true">
                <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor"
                    stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="4 17 10 11 4 5" />
                    <line x1="12" y1="19" x2="20" y2="19" />
                </svg>
            </span>
            <input
                ref={input_ref}
                class="query-bar__input"
                type="text"
                spellcheck="false"
                autocomplete="off"
                placeholder={r#"Filter — e.g. browser == "Chrome" && (country == "DE" || path like "/docs/*")"#}
                {value}
                {onfocus}
                {oninput}
                {onkeydown}
                aria-label="Filter query"
            />
            if *editing {
                <span class="query-bar__hint">{ "Enter to apply · Esc to cancel" }</span>
            }
            if let Some(message) = &*error {
                <div class="query-bar__error">{ message.clone() }</div>
            }
        </div>
    }
}

#[derive(Properties, PartialEq)]
pub struct AppBarProps {
    /// Toggles the navigation sidebar (rendered as an overlay on small
    /// screens; the toggle button only shows there).
    #[prop_or_default]
    pub on_menu: Callback<()>,
    /// Whether the mobile navigation overlay is open (for `aria-expanded`).
    #[prop_or(false)]
    pub nav_open: bool,
}

#[function_component(AppBar)]
pub fn app_bar(props: &AppBarProps) -> Html {
    let auth = use_context::<AuthHandle>().expect("AuthHandle context");
    let signed_in = matches!(auth.status, AuthStatus::SignedIn(_) | AuthStatus::Disabled);

    let on_menu = {
        let on_menu = props.on_menu.clone();
        Callback::from(move |_: MouseEvent| on_menu.emit(()))
    };

    let user = match &auth.user {
        Some(user) => {
            let on_signout = {
                let signout = auth.signout.clone();
                Callback::from(move |_: MouseEvent| signout.emit(()))
            };
            // Name and email travel in the tooltip; the bar itself stays lean.
            let title = match &user.email {
                Some(email) => format!("{} · {email}", user.name),
                None => user.name.clone(),
            };
            html! {
                <div class="user-chip" title={title}>
                    <span class="user-chip__avatar">{ initials(&user.name) }</span>
                    <span class="user-chip__name">{ user.name.clone() }</span>
                    <button class="user-chip__signout" onclick={on_signout}>{ "Sign out" }</button>
                </div>
            }
        }
        None => html! {},
    };

    html! {
        <header class="app-bar">
            if signed_in {
                <button class="app-bar__menu" onclick={on_menu}
                    aria-label="Toggle navigation" aria-expanded={props.nav_open.to_string()}>
                    { icons::menu() }
                </button>
            }
            <Link<Route> to={Route::Overview} classes="app-bar__brand">
                <img src="https://cdn.sierrasoftworks.com/logos/icon.svg" alt="Sierra Softworks" />
                <span class="app-bar__brand-name">{ "Analytics" }</span>
            </Link<Route>>
            if signed_in {
                <QueryBar />
            } else {
                <div class="app-bar__spacer" />
            }
            { user }
        </header>
    }
}
