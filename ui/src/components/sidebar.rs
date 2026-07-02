//! The left navigation sidebar: Dashboard, Exceptions, the project list (shown
//! only when projects exist), Tracking pixels, and Settings.
//!
//! Dashboard/Exceptions links carry the current filter state along so switching
//! views never silently drops active filters. Project entries apply a project
//! filter on the dashboard rather than navigating to a separate page.

use yew::prelude::*;
use yew_router::prelude::*;

use crate::app::Route;
use crate::components::ProjectsContext;
use crate::components::icons;
use crate::filters::{Dim, use_filters, use_navigate_with_filters};

#[function_component(Sidebar)]
pub fn sidebar() -> Html {
    let projects_ctx = use_context::<ProjectsContext>();
    let route = use_route::<Route>();
    let filters = use_filters();
    let navigate = use_navigate_with_filters();

    let projects = projects_ctx
        .as_ref()
        .map(|c| c.projects.clone())
        .unwrap_or_default();

    let is_dashboard = matches!(route, Some(Route::Overview | Route::Project { .. }));
    let is_exceptions = matches!(route, Some(Route::Exceptions | Route::Exception { .. }));
    let is_pixels = matches!(route, Some(Route::Pixels));
    let is_settings = matches!(route, Some(Route::Settings));
    let active_project = filters.get(Dim::Project).map(str::to_string);

    // Primary links keep the filter state; a page that can't honour a dimension
    // shows its chip as inert rather than dropping it.
    let menu_item = |active: bool, route: Route, icon: Html, label: &str| {
        let onclick = {
            let (navigate, filters) = (navigate.clone(), filters.clone());
            Callback::from(move |_: MouseEvent| navigate.emit((route.clone(), filters.clone())))
        };
        let class = classes!("menu__item", active.then_some("menu__item--active"));
        html! {
            <li>
                <button class={class} onclick={onclick}>
                    <span class="menu__icon">{ icon }</span>
                    <span class="menu__label">{ label.to_string() }</span>
                </button>
            </li>
        }
    };

    let project_items = projects
        .iter()
        .map(|p| {
            let active = is_dashboard && active_project.as_deref() == Some(p.id.as_str());
            let onclick = {
                let (navigate, filters, id) = (navigate.clone(), filters.clone(), p.id.clone());
                Callback::from(move |_: MouseEvent| {
                    navigate.emit((Route::Overview, filters.with(Dim::Project, id.clone())));
                })
            };
            let class = classes!("menu__subitem", active.then_some("menu__subitem--active"));
            html! {
                <li key={p.id.clone()}>
                    <button class={class} onclick={onclick}>
                        <span class="menu__dot" />
                        <span class="menu__label">{ &p.name }</span>
                    </button>
                </li>
            }
        })
        .collect::<Html>();

    html! {
        <nav class="app-sidebar">
            <ul class="menu">
                { menu_item(is_dashboard && active_project.is_none(), Route::Overview, icons::overview(), "Dashboard") }
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
