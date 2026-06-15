//! The global Exceptions inbox: recent crashes/failures across every project (and
//! unassigned sources), for triage and investigation.

use analytics_api::GlobalException;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::{ApiErrorAlert, PageHeader, Range, RangePicker};
use crate::pages::project::{status_class, status_label};
use crate::search::{MatchContext, SearchContext};

/// A compact "time ago" for the last-seen column.
fn ago(ms: i64) -> String {
    let now = js_sys::Date::now() as i64;
    let secs = ((now - ms) / 1000).max(0);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

#[function_component(Exceptions)]
pub fn exceptions() -> Html {
    let data = use_state(|| None::<Result<Vec<GlobalException>, ApiError>>);
    let range = use_state(Range::week);
    let filter = use_context::<SearchContext>()
        .map(|s| s.filter.clone())
        .unwrap_or_default();

    {
        let data = data.clone();
        let range = *range;
        use_effect_with(range, move |_| {
            spawn_local(async move {
                data.set(Some(api::list_all_exceptions(&range.query()).await));
            });
            || ()
        });
    }

    let set_range = {
        let range = range.clone();
        Callback::from(move |r: Range| range.set(r))
    };

    let body = match &*data {
        None => html! { <p class="muted">{ "Loading…" }</p> },
        Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
        Some(Ok(list)) if list.is_empty() => {
            html! { <div class="empty">{ "No exceptions reported in this period." }</div> }
        }
        Some(Ok(list)) => {
            let rows = list
                .iter()
                .filter(|e| {
                    let project = e.project_name.clone().unwrap_or_default();
                    let status = status_label(e.group.status).to_lowercase();
                    let source = e.source.to_lowercase();
                    let text = format!(
                        "{} {} {} {} {}",
                        e.group.exc_type, e.group.sample_message, project, status, e.source
                    )
                    .to_lowercase();
                    filter.matches(&MatchContext {
                        project: &project,
                        status: &status,
                        source: &source,
                        text: &text,
                        ..Default::default()
                    })
                })
                .map(row)
                .collect::<Html>();

            html! {
                <div class="card-table">
                    <table class="list">
                        <thead><tr>
                            <th>{ "Type" }</th><th>{ "Message" }</th><th>{ "Project" }</th>
                            <th>{ "Count" }</th><th>{ "Last seen" }</th><th>{ "Status" }</th>
                        </tr></thead>
                        <tbody>{ rows }</tbody>
                    </table>
                </div>
            }
        }
    };

    html! {
        <div class="page">
            <PageHeader title="Exceptions"
                subtitle="Recent crashes and errors across every project.">
                <RangePicker value={*range} on_change={set_range} />
            </PageHeader>
            { body }
        </div>
    }
}

fn row(e: &GlobalException) -> Html {
    // Link to the per-project detail only when the source is assigned to a project.
    let type_cell = match &e.project_id {
        Some(project) => html! {
            <Link<Route> to={Route::Exception { project: project.clone(), group: e.group.group_id.clone() }}>
                { &e.group.exc_type }
            </Link<Route>>
        },
        None => html! { <span>{ &e.group.exc_type }</span> },
    };
    let project = match &e.project_name {
        Some(name) => html! { { name } },
        None => html! { <span class="badge badge--muted">{ "Unassigned" }</span> },
    };

    html! {
        <tr>
            <td>{ type_cell }</td>
            <td class="ellipsis" title={e.group.sample_message.clone()}>{ &e.group.sample_message }</td>
            <td>{ project }</td>
            <td>{ e.group.count }</td>
            <td class="muted">{ ago(e.group.last_seen_ms) }</td>
            <td><span class={status_class(e.group.status)}>{ status_label(e.group.status) }</span></td>
        </tr>
    }
}
