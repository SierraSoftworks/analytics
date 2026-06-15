use analytics_api::{Overview as OverviewData, Project, SourceInput, source_label};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::{
    ApiErrorAlert, Dropdown, DropdownItem, MetricCards, PageHeader, ProjectsContext, Range,
    RangePicker, TimeSeriesChart,
};
use crate::search::{MatchContext, SearchContext};

#[function_component(Overview)]
pub fn overview() -> Html {
    let data = use_state(|| None::<Result<OverviewData, ApiError>>);
    let reload = use_state(|| 0u32);
    let range = use_state(Range::week);
    let projects_ctx = use_context::<ProjectsContext>();
    let filter = use_context::<SearchContext>()
        .map(|s| s.filter.clone())
        .unwrap_or_default();

    {
        let data = data.clone();
        let range = *range;
        use_effect_with((*reload, range), move |_| {
            spawn_local(async move {
                data.set(Some(api::overview(&range.query()).await));
            });
            || ()
        });
    }

    // Refresh both this page's data and the sidebar's project list.
    let bump = {
        let reload = reload.clone();
        let sidebar = projects_ctx.as_ref().map(|c| c.reload.clone());
        Callback::from(move |_: ()| {
            reload.set(*reload + 1);
            if let Some(sidebar) = &sidebar {
                sidebar.emit(());
            }
        })
    };

    let on_new = {
        let open_new = projects_ctx.as_ref().map(|c| c.open_new.clone());
        Callback::from(move |_: MouseEvent| {
            if let Some(open_new) = &open_new {
                open_new.emit(());
            }
        })
    };
    let set_range = {
        let range = range.clone();
        Callback::from(move |r: Range| range.set(r))
    };

    let projects = projects_ctx
        .as_ref()
        .map(|c| c.projects.clone())
        .unwrap_or_default();

    let body = match &*data {
        None => html! { <p class="muted">{ "Loading…" }</p> },
        Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
        Some(Ok(ov)) => {
            let filtered_projects: Vec<_> = ov
                .projects
                .iter()
                .filter(|p| {
                    let text = p.project.name.to_lowercase();
                    filter.matches(&MatchContext {
                        project: &p.project.name,
                        text: &text,
                        ..Default::default()
                    })
                })
                .collect();
            let filtered_unassigned: Vec<_> = ov
                .unassigned
                .iter()
                .filter(|u| {
                    let text = u.uri.to_lowercase();
                    filter.matches(&MatchContext {
                        source: &u.uri,
                        text: &text,
                        ..Default::default()
                    })
                })
                .collect();

            let project_rows = filtered_projects.iter().map(|p| html! {
                <tr>
                    <td>
                        <Link<Route> to={Route::Project { id: p.project.id.clone() }}>
                            { &p.project.name }
                        </Link<Route>>
                    </td>
                    <td>{ p.visitors }</td>
                    <td>{ p.pageviews }</td>
                </tr>
            }).collect::<Html>();
            let unassigned_rows = filtered_unassigned
                .iter()
                .map(|u| unassigned_row(&u.uri, u.visitors, u.pageviews, &projects, &bump))
                .collect::<Html>();

            html! {
                <>
                    <MetricCards summary={ov.summary.clone()} />
                    <div class="panel"><TimeSeriesChart points={ov.timeseries.clone()} /></div>

                    <section class="section">
                        <h2 class="section__title">{ "Projects" }</h2>
                        if ov.projects.is_empty() {
                            <div class="empty">{ "No projects yet. Create one to start grouping your sources." }</div>
                        } else if filtered_projects.is_empty() {
                            <div class="empty">{ "No projects match your search." }</div>
                        } else {
                            <div class="card-table">
                                <table class="list">
                                    <thead><tr><th>{ "Project" }</th><th>{ "Visitors" }</th><th>{ "Page views" }</th></tr></thead>
                                    <tbody>{ project_rows }</tbody>
                                </table>
                            </div>
                        }
                    </section>

                    if !ov.unassigned.is_empty() {
                        <section class="section">
                            <h2 class="section__title">{ "Unassigned sources" }</h2>
                            <p class="muted">{ "Reporting hostnames not yet grouped into a project." }</p>
                            if filtered_unassigned.is_empty() {
                                <div class="empty">{ "No sources match your search." }</div>
                            } else {
                                <div class="card-table">
                                    <table class="list">
                                        <thead><tr><th>{ "Source" }</th><th>{ "Visitors" }</th><th>{ "Page views" }</th><th>{ "Assign to" }</th></tr></thead>
                                        <tbody>{ unassigned_rows }</tbody>
                                    </table>
                                </div>
                            }
                        </section>
                    }
                </>
            }
        }
    };

    html! {
        <div class="page">
            <PageHeader title="Overview" subtitle="Traffic across every project.">
                <RangePicker value={*range} on_change={set_range} />
                <button class="btn btn--primary" onclick={on_new}>{ "New project" }</button>
            </PageHeader>
            { body }
        </div>
    }
}

fn unassigned_row(
    uri: &str,
    visitors: i64,
    pageviews: i64,
    projects: &[Project],
    bump: &Callback<()>,
) -> Html {
    let items: Vec<DropdownItem> =
        projects.iter().map(|p| DropdownItem::new(p.id.clone(), p.name.clone())).collect();
    let on_select = {
        let (uri, bump) = (uri.to_string(), bump.clone());
        Callback::from(move |project_id: String| {
            let input = SourceInput { project_id: Some(project_id), ..Default::default() };
            let (uri, bump) = (uri.clone(), bump.clone());
            spawn_local(async move {
                if api::update_source(&uri, &input).await.is_ok() {
                    bump.emit(());
                }
            });
        })
    };

    html! {
        <tr>
            <td><code>{ source_label(uri) }</code></td>
            <td>{ visitors }</td>
            <td>{ pageviews }</td>
            <td><Dropdown items={items} value="" placeholder="Assign…" on_select={on_select} /></td>
        </tr>
    }
}
