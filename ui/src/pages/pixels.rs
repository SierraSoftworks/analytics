//! The global Tracking Pixels page: every pixel across all projects. Creation and
//! editing (including the event-metadata payload) happen in a drawer.

use std::collections::BTreeMap;

use analytics_api::{Pixel, PixelInput};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yewi::prelude::{
    Alert, Button, ButtonVariant, ConfirmDialog, Copyable, Drawer, EmptyState, Field, IconButton,
    PageHeader, Select, SelectOption, SimpleTable, Size, Text, TextColor, TextInput, TextInputKind,
    Tone, VisuallyHidden, attrs, icons,
};

use crate::api::{self, ApiError};
use crate::components::page::loading;
use crate::components::{ApiErrorAlert, ProjectsContext};

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
    // The pixel awaiting a delete confirmation.
    let deleting = use_state(|| None::<Pixel>);

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
            projects
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| id.to_string())
        }
    };

    let open_new = {
        let (open, edit_id, f_name, f_event, f_project, f_meta, f_error) = (
            open.clone(),
            edit_id.clone(),
            f_name.clone(),
            f_event.clone(),
            f_project.clone(),
            f_meta.clone(),
            f_error.clone(),
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
            open.clone(),
            edit_id.clone(),
            f_name.clone(),
            f_event.clone(),
            f_project.clone(),
            f_meta.clone(),
            f_error.clone(),
        );
        move |pixel: &Pixel| {
            edit_id.set(Some(pixel.id.clone()));
            f_name.set(pixel.name.clone());
            f_event.set(pixel.event_name.clone());
            f_project.set(pixel.project_id.clone());
            f_meta.set(
                pixel
                    .metadata
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            );
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
            open.clone(),
            edit_id.clone(),
            f_name.clone(),
            f_event.clone(),
            f_project.clone(),
            f_meta.clone(),
            f_error.clone(),
            submitting.clone(),
            reload.clone(),
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
            let (open, f_error, submitting, reload) = (
                open.clone(),
                f_error.clone(),
                submitting.clone(),
                reload.clone(),
            );
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
        None => loading(),
        Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
        Some(Ok(list)) if list.is_empty() => {
            html! {
                <EmptyState title="No tracking pixels yet." icon={icons::IMAGE}>
                    { "A pixel counts opens where JavaScript cannot run: emails, RSS readers, READMEs." }
                </EmptyState>
            }
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
                    Callback::from(move |_: MouseEvent| open_edit(&pixel))
                };
                let on_delete = {
                    let (deleting, pixel) = (deleting.clone(), p.clone());
                    Callback::from(move |_: MouseEvent| deleting.set(Some(pixel.clone())))
                };
                html! {
                    <tr key={p.id.clone()}>
                        <th scope="row">{ &p.name }</th>
                        <td>{ project_name(&p.project_id) }</td>
                        <td>{ &p.event_name }</td>
                        <td class="pixel-url"><Copyable value={url} truncate=true label="Copy embed URL" /></td>
                        <td class="row-actions">
                            <IconButton icon={icons::PENCIL} label={format!("Edit {}", p.name)}
                                variant={ButtonVariant::Subtle} size={Size::Small} on_click={on_edit} />
                            <IconButton icon={icons::TRASH_2} label={format!("Delete {}", p.name)}
                                variant={ButtonVariant::Subtle} size={Size::Small} on_click={on_delete} />
                        </td>
                    </tr>
                }
            }).collect::<Html>();

            if visible.is_empty() {
                html! { <EmptyState title="No pixels match your search." icon={icons::SEARCH_X} /> }
            } else {
                html! {
                    <SimpleTable label="Tracking pixels">
                        <thead>
                            <tr>
                                <th scope="col">{ "Name" }</th>
                                <th scope="col">{ "Project" }</th>
                                <th scope="col">{ "Event" }</th>
                                <th scope="col">{ "Embed URL" }</th>
                                <th scope="col"><VisuallyHidden>{ "Actions" }</VisuallyHidden></th>
                            </tr>
                        </thead>
                        <tbody>{ rows }</tbody>
                    </SimpleTable>
                }
            }
        }
    };
    let on_needle = {
        let needle = needle.clone();
        Callback::from(move |value: String| needle.set(value))
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

    let on_confirm_delete = {
        let (deleting, reload) = (deleting.clone(), reload.clone());
        Callback::from(move |_: ()| {
            let Some(pixel) = (*deleting).clone() else {
                return;
            };
            let (deleting, reload) = (deleting.clone(), reload.clone());
            spawn_local(async move {
                if api::delete_pixel(&pixel.id).await.is_ok() {
                    reload.set(*reload + 1);
                }
                deleting.set(None);
            });
        })
    };
    let on_cancel_delete = {
        let deleting = deleting.clone();
        Callback::from(move |_: ()| deleting.set(None))
    };

    let actions = html! {
        <>
            <TextInput kind={TextInputKind::Search} placeholder="Filter pixels…" class="page-search"
                value={(*needle).clone()} on_input={on_needle}
                attrs={attrs! { "aria-label" => "Filter pixels" }} />
            <Button variant={ButtonVariant::Primary} icon={icons::PLUS} on_click={open_new} disabled={no_projects}>
                { "New pixel" }
            </Button>
        </>
    };

    html! {
        <div class="page">
            <PageHeader title="Tracking pixels" {actions}
                subtitle="1×1 GIF beacons for email opens, RSS, and other no-JavaScript contexts." />
            if no_projects {
                <Alert tone={Tone::Info}>{ "Create a project first. Every pixel belongs to one." }</Alert>
            }
            { body }
            { drawer }
            <ConfirmDialog open={deleting.is_some()} danger=true confirm_label="Delete pixel"
                title={format!("Delete {}?", deleting.as_ref().map(|p| p.name.as_str()).unwrap_or("this pixel"))}
                on_confirm={on_confirm_delete} on_cancel={on_cancel_delete}>
                { "Anything still embedding its URL stops being counted. This cannot be undone." }
            </ConfirmDialog>
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
        Callback::from(move |value: String| f.set(value))
    };
    let on_event = {
        let f = a.f_event.clone();
        Callback::from(move |value: String| f.set(value))
    };

    let project_field = if a.editing {
        html! {
            <Field label="Project" group=true>
                <Text color={TextColor::Muted}>{ (a.project_name)(&a.f_project) }</Text>
            </Field>
        }
    } else {
        let options: Vec<SelectOption> = a
            .projects
            .iter()
            .map(|p| SelectOption::new(p.id.clone(), p.name.clone()))
            .collect();
        let on_change = {
            let f = a.f_project.clone();
            Callback::from(move |v: AttrValue| f.set(v.to_string()))
        };
        html! {
            <Field label="Project">
                <Select {options} value={AttrValue::from((*a.f_project).clone())}
                    placeholder="Choose a project" {on_change} />
            </Field>
        }
    };

    // Metadata key/value editor.
    let meta_rows = (*a.f_meta).iter().enumerate().map(|(i, (k, v))| {
        let on_key = {
            let f = a.f_meta.clone();
            Callback::from(move |value: String| {
                let mut m = (*f).clone();
                if let Some(row) = m.get_mut(i) { row.0 = value; }
                f.set(m);
            })
        };
        let on_val = {
            let f = a.f_meta.clone();
            Callback::from(move |value: String| {
                let mut m = (*f).clone();
                if let Some(row) = m.get_mut(i) { row.1 = value; }
                f.set(m);
            })
        };
        let on_remove = {
            let f = a.f_meta.clone();
            Callback::from(move |_: MouseEvent| {
                let mut m = (*f).clone();
                if i < m.len() { m.remove(i); }
                f.set(m);
            })
        };
        html! {
            <div class="kv-editor__row" key={i.to_string()}>
                <TextInput placeholder="key" value={k.clone()} on_input={on_key} mono=true
                    attrs={attrs! { "aria-label" => format!("Field {} key", i + 1) }} />
                <TextInput placeholder="value" value={v.clone()} on_input={on_val}
                    attrs={attrs! { "aria-label" => format!("Field {} value", i + 1) }} />
                <IconButton icon={icons::MINUS} label="Remove field" variant={ButtonVariant::Subtle}
                    on_click={on_remove} />
            </div>
        }
    }).collect::<Html>();

    let on_add = {
        let f = a.f_meta.clone();
        Callback::from(move |_: MouseEvent| {
            let mut m = (*f).clone();
            m.push((String::new(), String::new()));
            f.set(m);
        })
    };

    let footer = {
        let on_cancel = a.on_close.reform(|_: MouseEvent| ());
        html! {
            <>
                <Button variant={ButtonVariant::Subtle} on_click={on_cancel}>{ "Cancel" }</Button>
                <Button variant={ButtonVariant::Primary} on_click={a.on_save} loading={a.submitting}>
                    { if a.editing { "Save changes" } else { "Create pixel" } }
                </Button>
            </>
        }
    };

    html! {
        <Drawer open={a.open} {title} on_close={a.on_close} {footer}>
            <div class="form">
                if let Some(error) = a.error {
                    <Alert tone={Tone::Danger}>{ error }</Alert>
                }
                <Field label="Name" required=true>
                    <TextInput placeholder="e.g. June newsletter" value={(*a.f_name).clone()} on_input={on_name} />
                </Field>
                <Field label="Event name" hint="Defaults to “pixel”.">
                    <TextInput placeholder="pixel" value={(*a.f_event).clone()} on_input={on_event} />
                </Field>
                { project_field }
                <Field label="Event metadata" group=true>
                    <div class="kv-editor">
                        { meta_rows }
                        <div><Button size={Size::Small} icon={icons::PLUS} on_click={on_add}>{ "Add field" }</Button></div>
                    </div>
                </Field>
            </div>
        </Drawer>
    }
}
