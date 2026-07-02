//! The global Tracking Pixels page: every pixel across all projects. Creation and
//! editing (including the event-metadata payload) happen in a drawer.

use std::collections::BTreeMap;

use analytics_api::{Pixel, PixelInput};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

use crate::api::{self, ApiError};
use crate::components::{ApiErrorAlert, Drawer, Dropdown, DropdownItem, PageHeader, ProjectsContext};

fn input_value(e: &InputEvent) -> String {
    e.target_unchecked_into::<HtmlInputElement>().value()
}

#[function_component(Pixels)]
pub fn pixels() -> Html {
    let pixels = use_state(|| None::<Result<Vec<Pixel>, ApiError>>);
    let reload = use_state(|| 0u32);
    let needle = use_state(String::new);
    let projects = use_context::<ProjectsContext>()
        .map(|c| c.projects.clone())
        .unwrap_or_default();

    // Drawer + form state (edit_id = None means "create").
    let open = use_state(|| false);
    let edit_id = use_state(|| None::<String>);
    let f_name = use_state(String::new);
    let f_event = use_state(String::new);
    let f_project = use_state(String::new);
    let f_meta = use_state(Vec::<(String, String)>::new);
    let f_error = use_state(|| None::<String>);
    let submitting = use_state(|| false);

    {
        let pixels = pixels.clone();
        use_effect_with(*reload, move |_| {
            spawn_local(async move {
                pixels.set(Some(api::list_all_pixels().await));
            });
            || ()
        });
    }

    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();

    let project_name = {
        let projects = projects.clone();
        move |id: &str| -> String {
            projects.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_else(|| id.to_string())
        }
    };

    let open_new = {
        let (open, edit_id, f_name, f_event, f_project, f_meta, f_error) = (
            open.clone(), edit_id.clone(), f_name.clone(), f_event.clone(),
            f_project.clone(), f_meta.clone(), f_error.clone(),
        );
        let first_project = projects.first().map(|p| p.id.clone()).unwrap_or_default();
        Callback::from(move |_: MouseEvent| {
            edit_id.set(None);
            f_name.set(String::new());
            f_event.set(String::new());
            f_project.set(first_project.clone());
            f_meta.set(Vec::new());
            f_error.set(None);
            open.set(true);
        })
    };

    let open_edit = {
        let (open, edit_id, f_name, f_event, f_project, f_meta, f_error) = (
            open.clone(), edit_id.clone(), f_name.clone(), f_event.clone(),
            f_project.clone(), f_meta.clone(), f_error.clone(),
        );
        move |pixel: &Pixel| {
            edit_id.set(Some(pixel.id.clone()));
            f_name.set(pixel.name.clone());
            f_event.set(pixel.event_name.clone());
            f_project.set(pixel.project_id.clone());
            f_meta.set(pixel.metadata.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
            f_error.set(None);
            open.set(true);
        }
    };

    let close = {
        let open = open.clone();
        Callback::from(move |_| open.set(false))
    };

    let on_save = {
        let (open, edit_id, f_name, f_event, f_project, f_meta, f_error, submitting, reload) = (
            open.clone(), edit_id.clone(), f_name.clone(), f_event.clone(), f_project.clone(),
            f_meta.clone(), f_error.clone(), submitting.clone(), reload.clone(),
        );
        Callback::from(move |_: MouseEvent| {
            let name = (*f_name).trim().to_string();
            if name.is_empty() {
                f_error.set(Some("A pixel name is required.".to_string()));
                return;
            }
            let mut metadata = BTreeMap::new();
            for (k, v) in (*f_meta).iter() {
                let key = k.trim();
                if !key.is_empty() {
                    metadata.insert(key.to_string(), v.clone());
                }
            }
            let input = PixelInput {
                name,
                event_name: Some((*f_event).trim().to_string()).filter(|e| !e.is_empty()),
                metadata,
            };
            let id = (*edit_id).clone();
            let project = (*f_project).clone();
            if id.is_none() && project.is_empty() {
                f_error.set(Some("Choose a project for the pixel.".to_string()));
                return;
            }
            let (open, f_error, submitting, reload) =
                (open.clone(), f_error.clone(), submitting.clone(), reload.clone());
            submitting.set(true);
            spawn_local(async move {
                let result = match &id {
                    Some(id) => api::update_pixel(id, &input).await,
                    None => api::create_pixel(&project, &input).await,
                };
                match result {
                    Ok(_) => {
                        reload.set(*reload + 1);
                        open.set(false);
                    }
                    Err(err) => f_error.set(Some(err.to_string())),
                }
                submitting.set(false);
            });
        })
    };

    let body = match &*pixels {
        None => html! { <p class="muted">{ "Loading…" }</p> },
        Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
        Some(Ok(list)) if list.is_empty() => {
            html! { <div class="empty">{ "No tracking pixels yet." }</div> }
        }
        Some(Ok(list)) => {
            let text = needle.to_lowercase();
            let visible: Vec<&Pixel> = list
                .iter()
                .filter(|p| {
                    text.is_empty()
                        || p.name.to_lowercase().contains(&text)
                        || p.event_name.to_lowercase().contains(&text)
                        || project_name(&p.project_id).to_lowercase().contains(&text)
                })
                .collect();
            let rows = visible.iter().map(|&p| {
                let url = format!("{origin}/track/gif/{}.gif", p.id);
                let on_edit = {
                    let (open_edit, pixel) = (open_edit.clone(), p.clone());
                    Callback::from(move |_| open_edit(&pixel))
                };
                let on_delete = {
                    let (pid, reload) = (p.id.clone(), reload.clone());
                    Callback::from(move |_| {
                        let (pid, reload) = (pid.clone(), reload.clone());
                        spawn_local(async move {
                            if api::delete_pixel(&pid).await.is_ok() {
                                reload.set(*reload + 1);
                            }
                        });
                    })
                };
                html! {
                    <tr>
                        <td>{ &p.name }</td>
                        <td>{ project_name(&p.project_id) }</td>
                        <td>{ &p.event_name }</td>
                        <td><code class="embed">{ url }</code></td>
                        <td class="row-actions">
                            <button class="btn btn--small btn--ghost" onclick={on_edit}>{ "Edit" }</button>
                            <button class="btn btn--small btn--ghost" onclick={on_delete}>{ "Delete" }</button>
                        </td>
                    </tr>
                }
            }).collect::<Html>();

            if visible.is_empty() {
                html! { <div class="empty">{ "No pixels match your search." }</div> }
            } else {
                html! {
                    <div class="card-table">
                        <table class="list">
                            <thead><tr><th>{ "Name" }</th><th>{ "Project" }</th><th>{ "Event" }</th><th>{ "Embed URL" }</th><th></th></tr></thead>
                            <tbody>{ rows }</tbody>
                        </table>
                    </div>
                }
            }
        }
    };
    let on_needle = {
        let needle = needle.clone();
        Callback::from(move |e: InputEvent| needle.set(input_value(&e)))
    };

    let no_projects = projects.is_empty();
    let drawer = pixel_drawer(PixelDrawerArgs {
        open: *open,
        editing: (*edit_id).is_some(),
        projects: &projects,
        project_name: &project_name,
        f_name: f_name.clone(),
        f_event: f_event.clone(),
        f_project: f_project.clone(),
        f_meta: f_meta.clone(),
        error: (*f_error).clone(),
        submitting: *submitting,
        on_close: close,
        on_save,
    });

    html! {
        <div class="page">
            <PageHeader title="Tracking pixels"
                subtitle="1×1 GIF beacons for email opens, RSS, and other no-JavaScript contexts.">
                <input class="input" type="search" placeholder="Filter pixels…"
                    value={(*needle).clone()} oninput={on_needle} />
                <button class="btn btn--primary" onclick={open_new} disabled={no_projects}>{ "New pixel" }</button>
            </PageHeader>
            if no_projects {
                <p class="muted">{ "Create a project first — every pixel belongs to one." }</p>
            }
            { body }
            { drawer }
        </div>
    }
}

struct PixelDrawerArgs<'a, F: Fn(&str) -> String> {
    open: bool,
    editing: bool,
    projects: &'a [analytics_api::Project],
    project_name: &'a F,
    f_name: UseStateHandle<String>,
    f_event: UseStateHandle<String>,
    f_project: UseStateHandle<String>,
    f_meta: UseStateHandle<Vec<(String, String)>>,
    error: Option<String>,
    submitting: bool,
    on_close: Callback<()>,
    on_save: Callback<MouseEvent>,
}

fn pixel_drawer<F: Fn(&str) -> String>(a: PixelDrawerArgs<F>) -> Html {
    let title = if a.editing { "Edit pixel" } else { "New pixel" };

    let on_name = {
        let f = a.f_name.clone();
        Callback::from(move |e: InputEvent| f.set(input_value(&e)))
    };
    let on_event = {
        let f = a.f_event.clone();
        Callback::from(move |e: InputEvent| f.set(input_value(&e)))
    };

    let project_field = if a.editing {
        html! {
            <div class="field">
                <label class="field__label">{ "Project" }</label>
                <p class="drawer__hint">{ (a.project_name)(&a.f_project) }</p>
            </div>
        }
    } else {
        let items: Vec<DropdownItem> =
            a.projects.iter().map(|p| DropdownItem::new(p.id.clone(), p.name.clone())).collect();
        let on_select = {
            let f = a.f_project.clone();
            Callback::from(move |v: String| f.set(v))
        };
        html! {
            <div class="field">
                <label class="field__label">{ "Project" }</label>
                <Dropdown block=true items={items} value={(*a.f_project).clone()}
                    placeholder="Choose a project" {on_select} />
            </div>
        }
    };

    // Metadata key/value editor.
    let meta_rows = (*a.f_meta).iter().enumerate().map(|(i, (k, v))| {
        let on_key = {
            let f = a.f_meta.clone();
            Callback::from(move |e: InputEvent| {
                let mut m = (*f).clone();
                if let Some(row) = m.get_mut(i) { row.0 = input_value(&e); }
                f.set(m);
            })
        };
        let on_val = {
            let f = a.f_meta.clone();
            Callback::from(move |e: InputEvent| {
                let mut m = (*f).clone();
                if let Some(row) = m.get_mut(i) { row.1 = input_value(&e); }
                f.set(m);
            })
        };
        let on_remove = {
            let f = a.f_meta.clone();
            Callback::from(move |_| {
                let mut m = (*f).clone();
                if i < m.len() { m.remove(i); }
                f.set(m);
            })
        };
        html! {
            <div class="kv-editor__row">
                <input class="input" placeholder="key" value={k.clone()} oninput={on_key} />
                <input class="input" placeholder="value" value={v.clone()} oninput={on_val} />
                <button class="kv-editor__remove" aria-label="Remove field" onclick={on_remove}>
                    <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor"
                        stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="5" y1="12" x2="19" y2="12" /></svg>
                </button>
            </div>
        }
    }).collect::<Html>();

    let on_add = {
        let f = a.f_meta.clone();
        Callback::from(move |_| {
            let mut m = (*f).clone();
            m.push((String::new(), String::new()));
            f.set(m);
        })
    };

    let error = a
        .error
        .map(|e| html! { <p class="drawer__hint" style="color: var(--danger);">{ e }</p> });

    let footer = {
        let on_cancel = {
            let close = a.on_close.clone();
            Callback::from(move |_: MouseEvent| close.emit(()))
        };
        let save_label = if a.submitting {
            "Saving…"
        } else if a.editing {
            "Save changes"
        } else {
            "Create pixel"
        };
        html! {
            <>
                <button class="btn btn--ghost" onclick={on_cancel}>{ "Cancel" }</button>
                <button class="btn btn--primary" onclick={a.on_save} disabled={a.submitting}>{ save_label }</button>
            </>
        }
    };

    html! {
        <Drawer open={a.open} title={title} on_close={a.on_close} footer={footer}>
            <div class="field">
                <label class="field__label">{ "Name" }</label>
                <input class="input" placeholder="e.g. June newsletter" value={(*a.f_name).clone()} oninput={on_name} />
            </div>
            <div class="field">
                <label class="field__label">{ "Event name" }</label>
                <input class="input" placeholder="default: pixel" value={(*a.f_event).clone()} oninput={on_event} />
            </div>
            { project_field }
            <div class="field">
                <label class="field__label">{ "Event metadata" }</label>
                <div class="kv-editor">
                    { meta_rows }
                    <button class="btn btn--small" onclick={on_add}>{ "Add field" }</button>
                </div>
            </div>
            { error }
        </Drawer>
    }
}
