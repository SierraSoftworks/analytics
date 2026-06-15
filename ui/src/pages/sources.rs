//! The per-project Sources tab: a summary of the sources assigned to this project,
//! plus a "Manage sources" drawer for toggling membership.

use analytics_api::{Source, SourceInput, SourceKind, source_label};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::api::{self, ApiError};
use crate::components::{ApiErrorAlert, Drawer};

/// A human label for a source kind (avoids leaking Rust `Debug` formatting).
fn kind_label(kind: &SourceKind) -> &'static str {
    match kind {
        SourceKind::Website => "Website",
        SourceKind::Application => "Application",
    }
}

#[derive(Properties, PartialEq)]
pub struct ProjectSourcesProps {
    pub id: String,
}

#[function_component(ProjectSources)]
pub fn project_sources(props: &ProjectSourcesProps) -> Html {
    let id = props.id.clone();
    let sources = use_state(|| None::<Result<Vec<Source>, ApiError>>);
    let reload = use_state(|| 0u32);
    let drawer_open = use_state(|| false);

    {
        let sources = sources.clone();
        use_effect_with(*reload, move |_| {
            spawn_local(async move {
                sources.set(Some(api::list_sources().await));
            });
            || ()
        });
    }

    // Assign (`Some(project)`) or release (`Some("")`, which the server unassigns).
    let set_project: Callback<(String, String)> = {
        let reload = reload.clone();
        Callback::from(move |(uri, project_id): (String, String)| {
            let reload = reload.clone();
            spawn_local(async move {
                let input = SourceInput { project_id: Some(project_id), ..Default::default() };
                if api::update_source(&uri, &input).await.is_ok() {
                    reload.set(*reload + 1);
                }
            });
        })
    };

    let open_drawer = {
        let drawer_open = drawer_open.clone();
        Callback::from(move |_| drawer_open.set(true))
    };
    let close_drawer = {
        let drawer_open = drawer_open.clone();
        Callback::from(move |_| drawer_open.set(false))
    };

    match &*sources {
        None => html! { <p class="muted">{ "Loading…" }</p> },
        Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
        Some(Ok(list)) => {
            let mine: Vec<&Source> =
                list.iter().filter(|s| s.project_id.as_deref() == Some(id.as_str())).collect();

            let assigned = if mine.is_empty() {
                html! { <div class="empty">{ "No sources assigned to this project yet." }</div> }
            } else {
                html! {
                    <div class="card-table">
                        <table class="list">
                            <thead><tr><th>{ "Source" }</th><th>{ "Kind" }</th></tr></thead>
                            <tbody>
                                { for mine.iter().map(|s| html! {
                                    <tr>
                                        <td><code>{ source_label(&s.uri) }</code></td>
                                        <td>{ kind_label(&s.kind) }</td>
                                    </tr>
                                }) }
                            </tbody>
                        </table>
                    </div>
                }
            };

            let rows = list.iter().map(|s| {
                let member = s.project_id.as_deref() == Some(id.as_str());
                let elsewhere = s.project_id.is_some() && !member;
                let onclick = {
                    let (set_project, uri, id) = (set_project.clone(), s.uri.clone(), id.clone());
                    Callback::from(move |_| {
                        let target = if member { String::new() } else { id.clone() };
                        set_project.emit((uri.clone(), target));
                    })
                };
                let (label, class) = if member {
                    ("Remove", "btn btn--small btn--ghost")
                } else {
                    ("Add", "btn btn--small")
                };
                html! {
                    <div class="toggle-row">
                        <span class="toggle-row__label" title={s.uri.clone()}>{ source_label(&s.uri) }</span>
                        if elsewhere {
                            <span class="badge badge--muted">{ "Assigned elsewhere" }</span>
                        }
                        <button class={class} {onclick}>{ label }</button>
                    </div>
                }
            }).collect::<Html>();

            html! {
                <>
                    <div class="form-row">
                        <button class="btn" onclick={open_drawer}>{ "Manage sources" }</button>
                    </div>
                    { assigned }
                    <Drawer open={*drawer_open} title="Manage sources" on_close={close_drawer}>
                        <p class="drawer__hint">{ "Toggle which reporting sources belong to this project." }</p>
                        if list.is_empty() {
                            <div class="empty">{ "No sources have reported yet." }</div>
                        } else {
                            { rows }
                        }
                    </Drawer>
                </>
            }
        }
    }
}
