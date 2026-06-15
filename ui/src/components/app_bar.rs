use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::app::{AuthHandle, AuthStatus, Route};
use crate::components::ProjectsContext;
use crate::components::icons;
use crate::search::{FIELD_PREFIXES, SearchContext, VocabularyContext};

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

fn input_value(event: &InputEvent) -> String {
    event
        .target_dyn_into::<HtmlInputElement>()
        .map(|input| input.value())
        .unwrap_or_default()
}

const MAX_SUGGESTIONS: usize = 8;

/// One autocomplete entry. Applying it either navigates (e.g. jumping to a
/// project) or rewrites the query text (field-prefix and value completions).
struct Suggestion {
    label: AttrValue,
    desc: Option<AttrValue>,
    /// The query string this produces when applied (used when `nav` is `None`).
    replacement: String,
    /// When set, applying this suggestion navigates instead of editing the query.
    nav: Option<Route>,
}

fn is_project_field(field: &str) -> bool {
    matches!(field.to_ascii_lowercase().as_str(), "project" | "proj" | "p")
}

/// The persistent top app bar: brand mark, the unified contextual search, and the
/// signed-in user chip.
#[function_component(AppBar)]
pub fn app_bar() -> Html {
    let auth = use_context::<AuthHandle>().expect("AuthHandle context");
    let search = use_context::<SearchContext>();
    let vocabulary = use_context::<VocabularyContext>();
    let projects = use_context::<ProjectsContext>();
    let navigator = use_navigator();

    let signed_in = matches!(auth.status, AuthStatus::SignedIn(_) | AuthStatus::Disabled);

    let highlight = use_state(|| None::<usize>);
    let dismissed = use_state(|| false);

    let search = match (signed_in, search) {
        (true, Some(search)) => {
            let query_str = search.query.to_string();
            let active_token = query_str.rsplit(char::is_whitespace).next().unwrap_or_default();
            let head = query_str[..query_str.len() - active_token.len()].to_string();

            let mut suggestions: Vec<Suggestion> = Vec::new();

            if active_token.is_empty() {
                // Nothing typed yet — no dropdown.
            } else if let Some((field, partial)) = active_token.split_once(':') {
                let needle = partial.to_lowercase();
                if is_project_field(field) {
                    // Jump straight to a matching project.
                    if let Some(projects) = projects.as_ref() {
                        for project in projects.projects.iter() {
                            if project.name.to_lowercase().contains(&needle) {
                                suggestions.push(Suggestion {
                                    label: AttrValue::from(project.name.clone()),
                                    desc: Some(AttrValue::from("Go to project")),
                                    replacement: String::new(),
                                    nav: Some(Route::Project { id: project.id.clone() }),
                                });
                            }
                        }
                    }
                } else if let Some(values) =
                    vocabulary.as_ref().and_then(|v| v.vocabulary.values_for(field))
                {
                    for value in values {
                        if value.to_lowercase().contains(&needle) {
                            suggestions.push(Suggestion {
                                label: value.clone(),
                                desc: None,
                                replacement: format!("{head}{field}:{value} "),
                                nav: None,
                            });
                        }
                    }
                }
            } else {
                // A bare term: offer matching projects to jump to, then the
                // `field:` prefixes (so the syntax is discoverable).
                let needle = active_token.to_lowercase();
                if let Some(projects) = projects.as_ref() {
                    for project in projects.projects.iter() {
                        if project.name.to_lowercase().contains(&needle) {
                            suggestions.push(Suggestion {
                                label: AttrValue::from(project.name.clone()),
                                desc: Some(AttrValue::from("Go to project")),
                                replacement: String::new(),
                                nav: Some(Route::Project { id: project.id.clone() }),
                            });
                        }
                    }
                }
                for (prefix, desc) in FIELD_PREFIXES {
                    if prefix.starts_with(needle.as_str()) {
                        suggestions.push(Suggestion {
                            label: AttrValue::from(*prefix),
                            desc: Some(AttrValue::from(*desc)),
                            replacement: format!("{head}{prefix}"),
                            nav: None,
                        });
                    }
                }
            }

            suggestions.truncate(MAX_SUGGESTIONS);
            let show = !suggestions.is_empty() && !*dismissed;

            // Pre-compute the apply action for each suggestion (nav or text).
            let actions: Vec<(Option<Route>, String)> =
                suggestions.iter().map(|s| (s.nav.clone(), s.replacement.clone())).collect();

            let apply = {
                let set = search.set.clone();
                let navigator = navigator.clone();
                let highlight = highlight.clone();
                move |action: &(Option<Route>, String)| {
                    highlight.set(None);
                    match &action.0 {
                        Some(route) => {
                            if let Some(nav) = &navigator {
                                nav.push(route);
                            }
                            set.emit(String::new());
                        }
                        None => set.emit(action.1.clone()),
                    }
                }
            };

            let oninput = {
                let set = search.set.clone();
                let highlight = highlight.clone();
                let dismissed = dismissed.clone();
                Callback::from(move |e: InputEvent| {
                    highlight.set(None);
                    dismissed.set(false);
                    set.emit(input_value(&e));
                })
            };

            let onkeydown = {
                let actions = actions.clone();
                let apply = apply.clone();
                let highlight = highlight.clone();
                let dismissed = dismissed.clone();
                Callback::from(move |e: KeyboardEvent| {
                    if actions.is_empty() {
                        return;
                    }
                    match e.key().as_str() {
                        "ArrowDown" => {
                            e.prevent_default();
                            let next = match *highlight {
                                Some(i) => (i + 1) % actions.len(),
                                None => 0,
                            };
                            highlight.set(Some(next));
                        }
                        "ArrowUp" => {
                            e.prevent_default();
                            let next = match *highlight {
                                Some(0) | None => actions.len() - 1,
                                Some(i) => i - 1,
                            };
                            highlight.set(Some(next));
                        }
                        "Enter" | "Tab" => {
                            if let Some(i) = *highlight {
                                e.prevent_default();
                                apply(&actions[i]);
                            }
                        }
                        "Escape" => {
                            e.prevent_default();
                            dismissed.set(true);
                            highlight.set(None);
                        }
                        _ => {}
                    }
                })
            };

            let dropdown = if show {
                let items = suggestions
                    .iter()
                    .enumerate()
                    .map(|(i, suggestion)| {
                        let active = *highlight == Some(i);
                        let onmousedown = {
                            let apply = apply.clone();
                            let action = actions[i].clone();
                            Callback::from(move |e: MouseEvent| {
                                e.prevent_default();
                                apply(&action);
                            })
                        };
                        let onmouseenter = {
                            let highlight = highlight.clone();
                            Callback::from(move |_: MouseEvent| highlight.set(Some(i)))
                        };
                        let class = classes!(
                            "app-bar__suggestion",
                            active.then_some("app-bar__suggestion--active")
                        );
                        let desc = match &suggestion.desc {
                            Some(desc) => {
                                html! { <span class="app-bar__suggestion-desc">{ desc.clone() }</span> }
                            }
                            None => html! {},
                        };
                        html! {
                            <li id={format!("search-opt-{i}")} class={class} role="option"
                                aria-selected={active.to_string()}
                                onmousedown={onmousedown} onmouseenter={onmouseenter}>
                                <span class="app-bar__suggestion-token">{ suggestion.label.clone() }</span>
                                { desc }
                            </li>
                        }
                    })
                    .collect::<Html>();
                html! { <ul id="search-suggestions" class="app-bar__suggestions" role="listbox">{ items }</ul> }
            } else {
                html! {}
            };

            html! {
                <div class="app-bar__search">
                    <span class="app-bar__search-icon">{ icons::search() }</span>
                    <input
                        type="search"
                        class="app-bar__search-input"
                        placeholder="Search projects, pages, sources…"
                        value={search.query.clone()}
                        oninput={oninput}
                        onkeydown={onkeydown}
                        role="combobox"
                        aria-label="Search"
                        aria-expanded={show.to_string()}
                        aria-autocomplete="list"
                        aria-controls="search-suggestions"
                        aria-activedescendant={(*highlight).map(|i| format!("search-opt-{i}")).unwrap_or_default()}
                    />
                    { dropdown }
                </div>
            }
        }
        _ => html! { <div class="app-bar__spacer" /> },
    };

    let user = match &auth.user {
        Some(user) => {
            let on_signout = {
                let signout = auth.signout.clone();
                Callback::from(move |_: MouseEvent| signout.emit(()))
            };
            let email = match &user.email {
                Some(email) => html! { <span class="user-chip__email">{ email.clone() }</span> },
                None => html! {},
            };
            html! {
                <div class="user-chip">
                    <span class="user-chip__avatar">{ initials(&user.name) }</span>
                    <span class="user-chip__meta">
                        <span class="user-chip__name">{ user.name.clone() }</span>
                        { email }
                    </span>
                    <button class="user-chip__signout" onclick={on_signout}>{ "Sign out" }</button>
                </div>
            }
        }
        None => html! {},
    };

    html! {
        <header class="app-bar">
            <Link<Route> to={Route::Overview} classes="app-bar__brand">
                <img src="https://cdn.sierrasoftworks.com/logos/icon.svg" alt="Sierra Softworks" />
                <span class="app-bar__brand-name">{ "Analytics" }</span>
                <span class="app-bar__brand-by">{ "by Sierra Softworks" }</span>
            </Link<Route>>
            { search }
            { user }
        </header>
    }
}
