use analytics_api::{ExceptionGroupDetail, ExceptionStatus, TriageInput};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::{ApiErrorAlert, Crumb, PageHeader};
use crate::pages::project::{status_class, status_label};

#[derive(Properties, PartialEq)]
pub struct ExceptionDetailProps {
    pub project: String,
    pub group: String,
}

#[function_component(ExceptionDetail)]
pub fn exception_detail(props: &ExceptionDetailProps) -> Html {
    let (project, group) = (props.project.clone(), props.group.clone());
    let detail = use_state(|| None::<Result<ExceptionGroupDetail, ApiError>>);
    let reload = use_state(|| 0u32);

    {
        let detail = detail.clone();
        let (project, group) = (project.clone(), group.clone());
        // `project` is a dependency: two projects can share a fingerprint, so
        // navigating between same-group exceptions across projects must refetch.
        use_effect_with((project.clone(), group.clone(), *reload), move |_| {
            spawn_local(async move {
                detail.set(Some(api::exception_detail(&group, &project).await));
            });
            || ()
        });
    }

    let set_status = {
        let (project, group, reload) = (project.clone(), group.clone(), reload.clone());
        move |status: ExceptionStatus| {
            let input = TriageInput { project_id: project.clone(), status, note: None };
            let (group, reload) = (group.clone(), reload.clone());
            Callback::from(move |_| {
                let (group, input, reload) = (group.clone(), input.clone(), reload.clone());
                spawn_local(async move {
                    if api::set_triage(&group, &input).await.is_ok() {
                        reload.set(*reload + 1);
                    }
                });
            })
        }
    };

    let title = match &*detail {
        Some(Ok(d)) => d.group.exc_type.clone(),
        _ => "Exception".to_string(),
    };
    let crumbs = vec![
        Crumb::link("Overview", Route::Overview),
        Crumb::link("Project", Route::Project { id: project.clone() }),
        Crumb::current(title.clone()),
    ];

    html! {
        <div class="page">
            <PageHeader crumbs={crumbs} title={title} />
            {
                match &*detail {
                    None => html! { <p class="muted">{ "Loading…" }</p> },
                    Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
                    Some(Ok(detail)) => html! {
                        <>
                            <div class="exc-header">
                                <span class={status_class(detail.group.status)}>{ status_label(detail.group.status) }</span>
                                <span class="muted">{ format!("{} occurrences", detail.group.count) }</span>
                            </div>
                            <p class="exc-message">{ &detail.group.sample_message }</p>
                            <div class="form-row">
                                <button class="btn" onclick={set_status(ExceptionStatus::Resolved)}>{ "Mark resolved" }</button>
                                <button class="btn" onclick={set_status(ExceptionStatus::Ignored)}>{ "Ignore" }</button>
                                <button class="btn btn--ghost" onclick={set_status(ExceptionStatus::Unresolved)}>{ "Reopen" }</button>
                            </div>
                            <h2 class="section__title">{ "Recent occurrences" }</h2>
                            { for detail.occurrences.iter().map(occurrence) }
                        </>
                    },
                }
            }
        </div>
    }
}

fn occurrence(o: &analytics_api::ExceptionOccurrence) -> Html {
    let ua = match (&o.ua_browser, &o.ua_os) {
        (Some(b), Some(os)) => format!("{b} on {os}"),
        (Some(b), None) => b.clone(),
        _ => "unknown client".to_string(),
    };
    html! {
        <div class="occurrence">
            <div class="occurrence__meta">{ format!("{} · {}", ua, if o.handled { "handled" } else { "unhandled" }) }</div>
            <div>{ &o.message }</div>
            if let Some(stack) = &o.stack {
                <pre class="stack">{ stack }</pre>
            }
        </div>
    }
}
