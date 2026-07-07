//! The signed-in application shell: the top app bar (brand · user), the left
//! navigation sidebar, and the routed page. It owns the shared project list and
//! the create-project drawer (openable anywhere via [`ProjectsContext::open_new`]).

use std::rc::Rc;

use analytics_api::{Project, ProjectInput, Source, SourceInput, source_label};
use chrono::{Datelike, Utc};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::app::Route;
use crate::components::{AppBar, Drawer, Sidebar};
use crate::filters::{Dim, FilterSet};

/// The project list shared by the sidebar, filter chips, and breakdown panels,
/// plus callbacks to refresh it and to open the create-project drawer.
#[derive(Clone, PartialEq)]
pub struct ProjectsContext {
    pub projects: Rc<Vec<Project>>,
    pub reload: Callback<()>,
    pub open_new: Callback<()>,
}

#[derive(Properties, PartialEq)]
pub struct AppShellProps {
    #[prop_or_default]
    pub children: Html,
}

/// A monotonic refresh counter driven through a reducer so that a *stable*
/// (memoized) reload callback still increments correctly — a plain `use_state`
/// handle captured in a `use_memo` would snapshot its value and only fire once.
#[derive(PartialEq)]
struct Tick(u32);
impl Reducible for Tick {
    type Action = ();
    fn reduce(self: Rc<Self>, _: ()) -> Rc<Self> {
        Rc::new(Tick(self.0.wrapping_add(1)))
    }
}

#[function_component(AppShell)]
pub fn app_shell(props: &AppShellProps) -> Html {
    let navigator = use_navigator();

    // The project list, refreshed whenever `tick` changes.
    let projects = use_state(|| Rc::new(Vec::<Project>::new()));
    let tick = use_reducer(|| Tick(0));
    {
        let projects = projects.clone();
        use_effect_with(tick.0, move |_| {
            spawn_local(async move {
                if let Ok(mut list) = api::list_projects().await {
                    list.sort_by_key(|p| p.name.to_lowercase());
                    projects.set(Rc::new(list));
                }
            });
            || ()
        });
    }
    let reload = {
        let dispatcher = tick.dispatcher();
        use_memo((), move |_| {
            Callback::from(move |_| dispatcher.dispatch(()))
        })
    };

    // The mobile navigation overlay (the sidebar is a fixed column on desktop).
    let nav_open = use_state(|| false);
    let toggle_nav = {
        let nav_open = nav_open.clone();
        Callback::from(move |_: ()| nav_open.set(!*nav_open))
    };
    let close_nav = {
        let nav_open = nav_open.clone();
        Callback::from(move |_: ()| nav_open.set(false))
    };

    // Create-project drawer state.
    let drawer_open = use_state(|| false);
    let new_name = use_state(String::new);
    let new_error = use_state(|| None::<String>);
    let submitting = use_state(|| false);
    let avail_sources = use_state(Vec::<Source>::new);
    let selected = use_state(Vec::<String>::new);

    let open_new = {
        let (drawer_open, new_name, new_error, selected, avail_sources) = (
            drawer_open.clone(),
            new_name.clone(),
            new_error.clone(),
            selected.clone(),
            avail_sources.clone(),
        );
        use_memo((), move |_| {
            let (drawer_open, new_name, new_error, selected, avail_sources) = (
                drawer_open.clone(),
                new_name.clone(),
                new_error.clone(),
                selected.clone(),
                avail_sources.clone(),
            );
            Callback::from(move |_| {
                new_name.set(String::new());
                new_error.set(None);
                selected.set(Vec::new());
                avail_sources.set(Vec::new()); // clear stale list so it doesn't flash
                drawer_open.set(true);
                let avail_sources = avail_sources.clone();
                spawn_local(async move {
                    if let Ok(list) = api::list_sources().await {
                        avail_sources.set(list);
                    }
                });
            })
        })
    };

    let projects_ctx = ProjectsContext {
        projects: (*projects).clone(),
        reload: (*reload).clone(),
        open_new: (*open_new).clone(),
    };

    let close_drawer = {
        let drawer_open = drawer_open.clone();
        Callback::from(move |_| drawer_open.set(false))
    };
    let on_name = {
        let new_name = new_name.clone();
        Callback::from(move |e: InputEvent| {
            new_name.set(e.target_unchecked_into::<HtmlInputElement>().value());
        })
    };
    let toggle_source = {
        let selected = selected.clone();
        Callback::from(move |uri: String| {
            let mut s = (*selected).clone();
            if let Some(pos) = s.iter().position(|u| u == &uri) {
                s.remove(pos);
            } else {
                s.push(uri);
            }
            selected.set(s);
        })
    };
    let on_create = {
        let (new_name, new_error, submitting, drawer_open, selected) = (
            new_name.clone(),
            new_error.clone(),
            submitting.clone(),
            drawer_open.clone(),
            selected.clone(),
        );
        let (dispatcher, navigator) = (tick.dispatcher(), navigator.clone());
        Callback::from(move |_| {
            let chosen = (*selected).clone();
            // Default the name to the first added source when left blank.
            let mut name = (*new_name).trim().to_string();
            if name.is_empty()
                && let Some(first) = chosen.first()
            {
                name = source_label(first).to_string();
            }
            if name.is_empty() {
                new_error.set(Some(
                    "Enter a project name, or add a source to name it after.".to_string(),
                ));
                return;
            }
            let (new_error, submitting, drawer_open) =
                (new_error.clone(), submitting.clone(), drawer_open.clone());
            let (dispatcher, navigator) = (dispatcher.clone(), navigator.clone());
            submitting.set(true);
            spawn_local(async move {
                match api::create_project(&ProjectInput { name, slug: None }).await {
                    Ok(project) => {
                        for uri in &chosen {
                            let input = SourceInput {
                                project_id: Some(project.id.clone()),
                                ..Default::default()
                            };
                            let _ = api::update_source(uri, &input).await;
                        }
                        dispatcher.dispatch(());
                        drawer_open.set(false);
                        // Land on the dashboard filtered to the new project
                        // (filters address projects by name).
                        if let Some(nav) = &navigator {
                            let filters = FilterSet::default().with(Dim::Project, project.name);
                            let _ = nav.push_with_query(&Route::Overview, &filters.to_pairs());
                        }
                    }
                    Err(err) => new_error.set(Some(err.to_string())),
                }
                submitting.set(false);
            });
        })
    };

    let error = (*new_error)
        .as_ref()
        .map(|e| html! { <p class="drawer__hint" style="color: var(--danger);">{ e.clone() }</p> });

    html! {
        <ContextProvider<ProjectsContext> context={projects_ctx}>
            <div class="app-shell">
                <AppBar on_menu={toggle_nav} nav_open={*nav_open} />
                <div class="app-body">
                    <Sidebar open={*nav_open} on_navigate={close_nav.clone()} />
                    if *nav_open {
                        <div class="app-scrim" onclick={{
                            let close_nav = close_nav.clone();
                            Callback::from(move |_: MouseEvent| close_nav.emit(()))
                        }} />
                    }
                    <main class="app-main">
                        <div class="app-content">{ props.children.clone() }</div>
                        <footer class="app-footer">
                            { format!("© Sierra Softworks {}", Utc::now().year()) }
                        </footer>
                    </main>
                </div>
            </div>
            <Drawer open={*drawer_open} title="New project" on_close={close_drawer.clone()}
                footer={drawer_footer(&close_drawer, &on_create, *submitting)}>
                <div class="field">
                    <label class="field__label">{ "Project name" }</label>
                    <input class="input" placeholder="Defaults to the first source added"
                        value={(*new_name).clone()} oninput={on_name} />
                </div>
                <div class="field">
                    <label class="field__label">{ "Add sources" }</label>
                    if avail_sources.is_empty() {
                        <p class="drawer__hint">{ "No reporting sources yet — they appear here once a site starts reporting." }</p>
                    } else {
                        { for (*avail_sources).iter().map(|s| {
                            let uri = s.uri.clone();
                            let chosen = (*selected).contains(&uri);
                            // Surface current ownership: adding a source already in
                            // another project will move it here.
                            let owner = s.project_id.as_ref()
                                .and_then(|pid| projects.iter().find(|p| &p.id == pid))
                                .map(|p| p.name.clone());
                            let onclick = {
                                let (toggle, uri) = (toggle_source.clone(), uri.clone());
                                Callback::from(move |_| toggle.emit(uri.clone()))
                            };
                            let (label, class) = if chosen {
                                ("Remove", "btn btn--small btn--ghost")
                            } else {
                                ("Add", "btn btn--small")
                            };
                            html! {
                                <div class="toggle-row">
                                    <span class="toggle-row__label" title={s.uri.clone()}>{ source_label(&s.uri) }</span>
                                    if let Some(owner) = owner {
                                        <span class="badge badge--muted">{ format!("in {owner}") }</span>
                                    }
                                    <button class={class} {onclick}>{ label }</button>
                                </div>
                            }
                        }) }
                    }
                </div>
                { error }
            </Drawer>
        </ContextProvider<ProjectsContext>>
    }
}

fn drawer_footer(close: &Callback<()>, create: &Callback<MouseEvent>, busy: bool) -> Html {
    let on_cancel = {
        let close = close.clone();
        Callback::from(move |_: MouseEvent| close.emit(()))
    };
    html! {
        <>
            <button class="btn btn--ghost" onclick={on_cancel}>{ "Cancel" }</button>
            <button class="btn btn--primary" onclick={create.clone()} disabled={busy}>
                { if busy { "Creating…" } else { "Create project" } }
            </button>
        </>
    }
}
