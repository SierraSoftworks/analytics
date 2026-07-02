//! The project management drawer: rename, source membership, and deletion —
//! management lives here so drilling into a project's data never leaves the
//! dashboard.

use analytics_api::{Project, ProjectInput, Source, SourceInput, source_label};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

use crate::api;
use crate::components::{Drawer, ProjectsContext};
use crate::filters::{Dim, use_apply_filters, use_filters};

#[derive(Properties, PartialEq)]
pub struct ProjectDrawerProps {
    /// The project being managed; `None` renders nothing.
    pub project_id: Option<String>,
    pub on_close: Callback<()>,
    /// Fired after any change (rename, membership, delete) so the page refetches.
    pub on_changed: Callback<()>,
}

#[function_component(ProjectDrawer)]
pub fn project_drawer(props: &ProjectDrawerProps) -> Html {
    let projects_ctx = use_context::<ProjectsContext>();
    let project = use_state(|| None::<Project>);
    let sources = use_state(Vec::<Source>::new);
    let name = use_state(String::new);
    let error = use_state(|| None::<String>);
    let busy = use_state(|| false);

    let filters = use_filters();
    let apply_filters = use_apply_filters();
    // Only the latest load may publish: rapidly switching managed projects must
    // not leave a previous project's name in the rename field.
    let load_seq = use_mut_ref(|| 0u64);

    // (Re)load the project + source list whenever the drawer opens.
    {
        let (project, sources, name, error, load_seq) = (
            project.clone(), sources.clone(), name.clone(), error.clone(), load_seq.clone(),
        );
        use_effect_with(props.project_id.clone(), move |id| {
            let seq = {
                let mut current = load_seq.borrow_mut();
                *current += 1;
                *current
            };
            error.set(None);
            if let Some(id) = id.clone() {
                spawn_local(async move {
                    let loaded = api::get_project(&id).await;
                    let source_list = api::list_sources().await;
                    if *load_seq.borrow() != seq {
                        return;
                    }
                    match loaded {
                        Ok(p) => {
                            name.set(p.name.clone());
                            project.set(Some(p));
                        }
                        Err(err) => error.set(Some(err.to_string())),
                    }
                    if let Ok(list) = source_list {
                        sources.set(list);
                    }
                });
            } else {
                project.set(None);
            }
            || ()
        });
    }

    let Some(id) = props.project_id.clone() else {
        return html! {};
    };

    let reload_projects = projects_ctx.as_ref().map(|c| c.reload.clone());
    let notify = {
        let (on_changed, reload_projects) = (props.on_changed.clone(), reload_projects.clone());
        Callback::from(move |_: ()| {
            on_changed.emit(());
            if let Some(reload) = &reload_projects {
                reload.emit(());
            }
        })
    };

    let on_name = {
        let name = name.clone();
        Callback::from(move |e: InputEvent| {
            name.set(e.target_unchecked_into::<HtmlInputElement>().value());
        })
    };

    let on_rename = {
        let (id, name, error, busy, notify) =
            (id.clone(), name.clone(), error.clone(), busy.clone(), notify.clone());
        Callback::from(move |_: MouseEvent| {
            let new_name = name.trim().to_string();
            if new_name.is_empty() {
                error.set(Some("Enter a project name.".to_string()));
                return;
            }
            let (id, error, busy, notify) =
                (id.clone(), error.clone(), busy.clone(), notify.clone());
            busy.set(true);
            spawn_local(async move {
                let input = ProjectInput { name: new_name, slug: None };
                match api::update_project(&id, &input).await {
                    Ok(_) => notify.emit(()),
                    Err(err) => error.set(Some(err.to_string())),
                }
                busy.set(false);
            });
        })
    };

    // Toggle a source in/out of this project.
    let toggle_source = {
        let (id, sources, error, notify) =
            (id.clone(), sources.clone(), error.clone(), notify.clone());
        Callback::from(move |(uri, member): (String, bool)| {
            let target = if member { String::new() } else { id.clone() };
            let (sources, error, notify) = (sources.clone(), error.clone(), notify.clone());
            spawn_local(async move {
                let input = SourceInput { project_id: Some(target), ..Default::default() };
                match api::update_source(&uri, &input).await {
                    Ok(_) => {
                        if let Ok(list) = api::list_sources().await {
                            sources.set(list);
                        }
                        notify.emit(());
                    }
                    Err(err) => error.set(Some(err.to_string())),
                }
            });
        })
    };

    let on_delete = {
        let (id, error, busy, notify, on_close) = (
            id.clone(),
            error.clone(),
            busy.clone(),
            notify.clone(),
            props.on_close.clone(),
        );
        let (filters, apply_filters) = (filters.clone(), apply_filters.clone());
        Callback::from(move |_: MouseEvent| {
            let confirmed = web_sys::window()
                .and_then(|w| {
                    w.confirm_with_message(
                        "Delete this project? Its pixels are removed and its sources become unassigned. This cannot be undone.",
                    )
                    .ok()
                })
                .unwrap_or(false);
            if !confirmed {
                return;
            }
            let (id, error, busy, notify, on_close) = (
                id.clone(),
                error.clone(),
                busy.clone(),
                notify.clone(),
                on_close.clone(),
            );
            let (filters, apply_filters) = (filters.clone(), apply_filters.clone());
            busy.set(true);
            spawn_local(async move {
                match api::delete_project(&id).await {
                    Ok(()) => {
                        notify.emit(());
                        on_close.emit(());
                        // Don't strand the dashboard filtered to a project that
                        // no longer exists.
                        if filters.get(Dim::Project) == Some(id.as_str()) {
                            apply_filters.emit(filters.without(Dim::Project));
                        }
                    }
                    Err(err) => error.set(Some(err.to_string())),
                }
                busy.set(false);
            });
        })
    };

    let title = project.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "Project".to_string());
    let on_close = {
        let on_close = props.on_close.clone();
        Callback::from(move |_: ()| on_close.emit(()))
    };

    let source_rows = sources.iter().map(|s| {
        let member = s.project_id.as_deref() == Some(id.as_str());
        let elsewhere = s.project_id.is_some() && !member;
        let onclick = {
            let (toggle, uri) = (toggle_source.clone(), s.uri.clone());
            Callback::from(move |_: MouseEvent| toggle.emit((uri.clone(), member)))
        };
        let (label, class) = if member {
            ("Remove", "btn btn--small btn--ghost")
        } else {
            ("Add", "btn btn--small")
        };
        html! {
            <div class="toggle-row" key={s.uri.clone()}>
                <span class="toggle-row__label" title={s.uri.clone()}>{ source_label(&s.uri) }</span>
                if elsewhere {
                    <span class="badge badge--muted">{ "Assigned elsewhere" }</span>
                }
                <button class={class} {onclick}>{ label }</button>
            </div>
        }
    });

    html! {
        <Drawer open={true} title={title} on_close={on_close}>
            if let Some(error) = &*error {
                <p class="drawer__hint" style="color: var(--danger);">{ error.clone() }</p>
            }
            <div class="field">
                <label class="field__label">{ "Project name" }</label>
                <div class="form-row" style="margin: 0;">
                    <input class="input" style="flex: 1;" value={(*name).clone()} oninput={on_name} />
                    <button class="btn" onclick={on_rename} disabled={*busy}>{ "Rename" }</button>
                </div>
            </div>
            <div class="field">
                <label class="field__label">{ "Sources" }</label>
                if sources.is_empty() {
                    <p class="drawer__hint">{ "No reporting sources yet — they appear here once a site starts reporting." }</p>
                } else {
                    { for source_rows }
                }
            </div>
            <div class="field">
                <label class="field__label">{ "Danger zone" }</label>
                <p class="drawer__hint">
                    { "Deleting a project removes its tracking pixels and unassigns its sources. Historical events are retained under those sources." }
                </p>
                <div>
                    <button class="btn btn--danger" onclick={on_delete} disabled={*busy}>
                        { "Delete project" }
                    </button>
                </div>
            </div>
        </Drawer>
    }
}
