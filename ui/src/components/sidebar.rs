//! The left navigation sidebar: Overview, Exceptions, the project list (shown only
//! when projects exist), Tracking pixels, and Settings. The active route is
//! highlighted; the project list comes from the shared [`ProjectsContext`]. Project
//! creation lives in the Overview header, not here.

use yew::prelude::*;
use yew_router::prelude::*;

use crate::app::Route;
use crate::components::ProjectsContext;
use crate::components::icons;

#[function_component(Sidebar)]
pub fn sidebar() -> Html {
    let projects_ctx = use_context::<ProjectsContext>();
    let route = use_route::<Route>();

    let projects = projects_ctx
        .as_ref()
        .map(|c| c.projects.clone())
        .unwrap_or_default();

    let is_overview = matches!(route, Some(Route::Overview));
    let is_exceptions = matches!(route, Some(Route::Exceptions));
    let is_pixels = matches!(route, Some(Route::Pixels));
    let is_settings = matches!(route, Some(Route::Settings));
    let active_project = match &route {
        Some(Route::Project { id }) => Some(id.clone()),
        Some(Route::Exception { project, .. }) => Some(project.clone()),
        _ => None,
    };

    let menu_item = |active: bool, route: Route, icon: Html, label: &str| {
        let class = classes!("menu__item", active.then_some("menu__item--active"));
        html! {
            <li>
                <Link<Route> to={route} classes={class}>
                    <span class="menu__icon">{ icon }</span>
                    <span class="menu__label">{ label.to_string() }</span>
                </Link<Route>>
            </li>
        }
    };

    let project_items = projects.iter().map(|p| {
        let active = active_project.as_deref() == Some(p.id.as_str());
        let class = classes!("menu__subitem", active.then_some("menu__subitem--active"));
        html! {
            <li>
                <Link<Route> to={Route::Project { id: p.id.clone() }} classes={class}>
                    <span class="menu__dot" />
                    <span class="menu__label">{ &p.name }</span>
                </Link<Route>>
            </li>
        }
    }).collect::<Html>();

    html! {
        <nav class="app-sidebar">
            <ul class="menu">
                { menu_item(is_overview, Route::Overview, icons::overview(), "Overview") }
                { menu_item(is_exceptions, Route::Exceptions, icons::exceptions(), "Exceptions") }

                if !projects.is_empty() {
                    <li class="menu__section">{ "Projects" }</li>
                    <ul class="menu__sub">{ project_items }</ul>
                }

                <li class="menu__section">{ "Manage" }</li>
                { menu_item(is_pixels, Route::Pixels, icons::pixels(), "Tracking pixels") }
                { menu_item(is_settings, Route::Settings, icons::settings(), "Settings") }
            </ul>
        </nav>
    }
}
