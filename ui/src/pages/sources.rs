use analytics_api::{Project, Source, SourceInput, source_label};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlSelectElement;
use yew::prelude::*;

use crate::api::{self, ApiError};

#[function_component(Sources)]
pub fn sources() -> Html {
    let projects = use_state(Vec::<Project>::new);
    let sources = use_state(|| None::<Result<Vec<Source>, ApiError>>);
    let reload = use_state(|| 0u32);

    {
        let (projects, sources) = (projects.clone(), sources.clone());
        use_effect_with(*reload, move |_| {
            spawn_local(async move {
                if let Ok(list) = api::list_projects().await {
                    projects.set(list);
                }
                sources.set(Some(api::list_sources().await));
            });
            || ()
        });
    }

    html! {
        <div class="page">
            <h1>{ "Sources" }</h1>
            <p class="muted">{ "Group reporting hostnames into projects." }</p>
            {
                match &*sources {
                    None => html! { <p class="muted">{ "Loading…" }</p> },
                    Some(Err(err)) => html! { <div class="alert alert--error">{ err.to_string() }</div> },
                    Some(Ok(list)) if list.is_empty() => html! {
                        <p class="muted">{ "No sources yet. They appear automatically once a site starts reporting." }</p>
                    },
                    Some(Ok(list)) => html! {
                        <table class="list">
                            <thead><tr><th>{ "Source" }</th><th>{ "Kind" }</th><th>{ "Project" }</th><th></th></tr></thead>
                            <tbody>
                            { for list.iter().map(|s| source_row(s, &projects, &reload)) }
                            </tbody>
                        </table>
                    },
                }
            }
        </div>
    }
}

fn source_row(source: &Source, projects: &[Project], reload: &UseStateHandle<u32>) -> Html {
    let uri = source.uri.clone();
    let current = source.project_id.clone().unwrap_or_default();

    let on_assign = {
        let (uri, reload) = (uri.clone(), reload.clone());
        Callback::from(move |e: Event| {
            let select: HtmlSelectElement = e.target_unchecked_into();
            let project_id = select.value();
            let input = SourceInput {
                project_id: Some(project_id),
                ..Default::default()
            };
            let (uri, reload) = (uri.clone(), reload.clone());
            spawn_local(async move {
                if api::update_source(&uri, &input).await.is_ok() {
                    reload.set(*reload + 1);
                }
            });
        })
    };

    let on_delete = {
        let (uri, reload) = (uri.clone(), reload.clone());
        Callback::from(move |_| {
            let (uri, reload) = (uri.clone(), reload.clone());
            spawn_local(async move {
                if api::delete_source(&uri).await.is_ok() {
                    reload.set(*reload + 1);
                }
            });
        })
    };

    html! {
        <tr>
            <td><code>{ source_label(&source.uri) }</code></td>
            <td>{ format!("{:?}", source.kind).to_lowercase() }</td>
            <td>
                <select class="input" onchange={on_assign}>
                    <option value="" selected={current.is_empty()}>{ "Unassigned" }</option>
                    { for projects.iter().map(|p| html! {
                        <option value={p.id.clone()} selected={p.id == current}>{ &p.name }</option>
                    }) }
                </select>
            </td>
            <td><button class="btn btn--ghost" onclick={on_delete}>{ "Delete" }</button></td>
        </tr>
    }
}
