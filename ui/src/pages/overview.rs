use analytics_api::{Overview as OverviewData, ProjectInput};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::{ApiErrorAlert, MetricCards, TimeSeriesChart};

#[function_component(Overview)]
pub fn overview() -> Html {
    let data = use_state(|| None::<Result<OverviewData, ApiError>>);
    let reload = use_state(|| 0u32);
    let new_name = use_state(String::new);

    {
        let data = data.clone();
        use_effect_with(*reload, move |_| {
            spawn_local(async move {
                data.set(Some(api::overview("interval=day").await));
            });
            || ()
        });
    }

    let on_name = {
        let new_name = new_name.clone();
        Callback::from(move |e: InputEvent| {
            let input: HtmlInputElement = e.target_unchecked_into();
            new_name.set(input.value());
        })
    };
    let on_create = {
        let (new_name, reload) = (new_name.clone(), reload.clone());
        Callback::from(move |_| {
            let name = (*new_name).trim().to_string();
            if name.is_empty() {
                return;
            }
            let (new_name, reload) = (new_name.clone(), reload.clone());
            spawn_local(async move {
                if api::create_project(&ProjectInput { name, slug: None }).await.is_ok() {
                    new_name.set(String::new());
                    reload.set(*reload + 1);
                }
            });
        })
    };

    match &*data {
        None => html! { <p class="muted">{ "Loading…" }</p> },
        Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
        Some(Ok(overview)) => html! {
            <div class="page">
                <h1>{ "Overview" }</h1>
                <MetricCards summary={overview.summary.clone()} />
                <div class="panel">
                    <TimeSeriesChart points={overview.timeseries.clone()} />
                </div>

                <h2>{ "Projects" }</h2>
                <div class="form-row">
                    <input class="input" placeholder="New project name" value={(*new_name).clone()} oninput={on_name} />
                    <button class="btn btn--primary" onclick={on_create}>{ "Create project" }</button>
                </div>
                if overview.projects.is_empty() {
                    <p class="muted">{ "No projects yet." }</p>
                } else {
                    <table class="list">
                        <thead><tr><th>{ "Project" }</th><th>{ "Visitors" }</th><th>{ "Page views" }</th><th></th></tr></thead>
                        <tbody>
                        { for overview.projects.iter().map(|p| {
                            let on_delete = {
                                let (id, reload) = (p.project.id.clone(), reload.clone());
                                Callback::from(move |_| {
                                    let (id, reload) = (id.clone(), reload.clone());
                                    spawn_local(async move {
                                        if api::delete_project(&id).await.is_ok() {
                                            reload.set(*reload + 1);
                                        }
                                    });
                                })
                            };
                            html! {
                                <tr>
                                    <td><Link<Route> to={Route::Project { id: p.project.id.clone() }}>{ &p.project.name }</Link<Route>></td>
                                    <td>{ p.visitors }</td>
                                    <td>{ p.pageviews }</td>
                                    <td><button class="btn btn--ghost" onclick={on_delete}>{ "Delete" }</button></td>
                                </tr>
                            }
                        }) }
                        </tbody>
                    </table>
                }

                if !overview.unassigned.is_empty() {
                    <>
                        <h2>{ "Unassigned sources" }</h2>
                        <p class="muted">{ "Reporting hostnames not yet grouped into a project." }</p>
                        <table class="list">
                            <thead><tr><th>{ "Source" }</th><th>{ "Visitors" }</th><th>{ "Page views" }</th></tr></thead>
                            <tbody>
                            { for overview.unassigned.iter().map(|u| html! {
                                <tr><td>{ &u.uri }</td><td>{ u.visitors }</td><td>{ u.pageviews }</td></tr>
                            }) }
                            </tbody>
                        </table>
                        <p><Link<Route> to={Route::Sources}>{ "Manage sources →" }</Link<Route>></p>
                    </>
                }
            </div>
        },
    }
}
