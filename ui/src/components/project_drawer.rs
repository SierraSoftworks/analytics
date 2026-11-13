//! The project management drawer: rename, source membership, and deletion —
//! management lives here so drilling into a project's data never leaves the
//! dashboard.

use analytics_api::{Project, ProjectInput, Source, SourceInput, source_label};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yewi::prelude::{
    Alert, Button, ButtonVariant, ConfirmDialog, Drawer, Field, Pill, Size, TextInput, Tone,
};

use crate::api;
use crate::components::ProjectsContext;
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
    let confirming = use_state(|| false);

    let filters = use_filters();
    let apply_filters = use_apply_filters();
    // Only the latest load may publish: rapidly switching managed projects must
    // not leave a previous project's name in the rename field.
    let load_seq = use_mut_ref(|| 0u64);

    // (Re)load the project + source list whenever the drawer opens.
    {
        let (project, sources, name, error, load_seq) = (
            project.clone(),
            sources.clone(),
            name.clone(),
            error.clone(),
            load_seq.clone(),
        );
        let confirming = confirming.clone();
        use_effect_with(props.project_id.clone(), move |id| {
            confirming.set(false);
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
        Callback::from(move |value: String| name.set(value))
    };

    let on_rename = {
        let (id, name, error, busy, notify) = (
            id.clone(),
            name.clone(),
            error.clone(),
            busy.clone(),
            notify.clone(),
        );
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
                let input = ProjectInput {
                    name: new_name,
                    slug: None,
                };
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
                let input = SourceInput {
                    project_id: Some(target),
                    ..Default::default()
                };
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
        let (project, confirming) = (project.clone(), confirming.clone());
        Callback::from(move |_: ()| {
            confirming.set(false);
            let (id, error, busy, notify, on_close) = (
                id.clone(),
                error.clone(),
                busy.clone(),
                notify.clone(),
                on_close.clone(),
            );
            let (filters, apply_filters) = (filters.clone(), apply_filters.clone());
            // Filters address projects by name (case-insensitively; old links
            // by id) — capture the name so deletion can clear a matching chip.
            let name = project.as_ref().map(|p| p.name.to_lowercase());
            busy.set(true);
            spawn_local(async move {
                match api::delete_project(&id).await {
                    Ok(()) => {
                        notify.emit(());
                        on_close.emit(());
                        // Don't strand the dashboard filtered to a project that
                        // no longer exists.
                        let filtered = filters
                            .get(Dim::Project)
                            .is_some_and(|v| v == id.as_str() || Some(v.to_lowercase()) == name);
                        if filtered {
                            apply_filters.emit(filters.without(Dim::Project));
                        }
                    }
                    Err(err) => error.set(Some(err.to_string())),
                }
                busy.set(false);
            });
        })
    };

    let title = project
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Project".to_string());
    let on_close = {
        let on_close = props.on_close.clone();
        Callback::from(move |_: ()| on_close.emit(()))
    };

    let source_rows = sources.iter().map(|s| {
        let member = s.project_id.as_deref() == Some(id.as_str());
        let elsewhere = s.project_id.is_some() && !member;
        let on_click = {
            let (toggle, uri) = (toggle_source.clone(), s.uri.clone());
            Callback::from(move |_: MouseEvent| toggle.emit((uri.clone(), member)))
        };
        html! {
            <li class="choice-row" key={s.uri.clone()}>
                <span class="choice-row__label" title={s.uri.clone()}>{ source_label(&s.uri) }</span>
                if elsewhere {
                    <Pill dot=false>{ "Assigned elsewhere" }</Pill>
                }
                <Button size={Size::Small} {on_click}
                    variant={if member { ButtonVariant::Subtle } else { ButtonVariant::Default }}>
                    { if member { "Remove" } else { "Add" } }
                </Button>
            </li>
        }
    });

    let ask_delete = {
        let confirming = confirming.clone();
        Callback::from(move |_: MouseEvent| confirming.set(true))
    };
    let cancel_delete = {
        let confirming = confirming.clone();
        Callback::from(move |_: ()| confirming.set(false))
    };

    html! {
        <>
            <Drawer open=true {title} {on_close}>
                <div class="form">
                    if let Some(error) = &*error {
                        <Alert tone={Tone::Danger}>{ error.clone() }</Alert>
                    }
                    <Field label="Project name">
                        <div class="form__row">
                            <TextInput value={(*name).clone()} on_input={on_name} />
                            <Button on_click={on_rename} disabled={*busy}>{ "Rename" }</Button>
                        </div>
                    </Field>
                    <Field label="Sources" group=true
                        hint={sources.is_empty().then_some(
                            "No reporting sources yet. They appear here once a site starts reporting."
                        )}>
                        <ul class="choice-rows">{ for source_rows }</ul>
                    </Field>
                    <Field label="Danger zone" group=true
                        hint="Deleting a project removes its tracking pixels and unassigns its sources. Historical events are retained under those sources.">
                        <div>
                            <Button variant={ButtonVariant::Danger} on_click={ask_delete} disabled={*busy}>
                                { "Delete project" }
                            </Button>
                        </div>
                    </Field>
                </div>
            </Drawer>
            <ConfirmDialog open={*confirming} title="Delete this project?" confirm_label="Delete project"
                danger=true busy={*busy} on_confirm={on_delete} on_cancel={cancel_delete}>
                { "Its pixels are removed and its sources become unassigned. This cannot be undone." }
            </ConfirmDialog>
        </>
    }
}
