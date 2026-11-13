//! The signed-in application shell: the app bar (brand · filter query · user),
//! the navigation sidebar, and the routed page. It owns the shared project list
//! and the create-project drawer (openable anywhere via
//! [`ProjectsContext::open_new`]).

use std::rc::Rc;

use analytics_api::{Project, ProjectInput, Source, SourceInput, source_label};
use chrono::{Datelike, Utc};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewi::prelude::{
    Alert, AppBar, AppBarBrand, AppShell as Shell, Button, ButtonVariant, Drawer, Field, NavTarget,
    Pill, QueryField, QueryInput, QuerySyntax, Sidebar, SidebarGroup, SidebarItem, Size, Text,
    TextColor, TextInput, TextSize, TextTag, ThemeToggle, Tone, UserMenu, icons,
};

use crate::api;
use crate::app::{AuthHandle, Route};
use crate::components::SuggestOption;
use crate::filters::{Dim, FilterSet, Query, use_apply_filters, use_filters, validate_expression};

/// The project list shared by the sidebar, filter chips, and breakdown panels,
/// plus callbacks to refresh it and to open the create-project drawer.
#[derive(Clone, PartialEq)]
pub struct ProjectsContext {
    pub projects: Rc<Vec<Project>>,
    pub reload: Callback<()>,
    pub open_new: Callback<()>,
}

/// The values worth suggesting per filter dimension. The routed page knows
/// them (its breakdown rows); the query bar in the app bar offers them.
pub type Suggestions = Rc<Vec<(Dim, Vec<SuggestOption>)>>;

/// Lets the routed page publish its [`Suggestions`] to the query bar.
#[derive(Clone, PartialEq)]
pub struct SuggestionsContext {
    pub set: Callback<Suggestions>,
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

/// The brand mark at the start of the app bar, linking back to the dashboard.
pub fn brand() -> Html {
    let logo = html! { <img src="https://cdn.sierrasoftworks.com/logos/icon.svg" alt="" /> };
    html! {
        <AppBarBrand {logo} href={NavTarget::route(Route::Overview)}>{ "Analytics" }</AppBarBrand>
    }
}

/// The copyright line at the foot of every page.
pub fn footer() -> Html {
    html! {
        <footer class="app-footer">
            <Text tag={TextTag::P} size={TextSize::Xs} color={TextColor::Subtle}>
                { format!("© Sierra Softworks {}", Utc::now().year()) }
            </Text>
        </footer>
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

    // The query bar's value suggestions, published by the routed page. The
    // setter is memoised so the context identity stays stable.
    let suggestions = use_state_eq(Suggestions::default);
    let suggestions_ctx = {
        let suggestions = suggestions.clone();
        use_memo((), move |_| SuggestionsContext {
            set: Callback::from(move |value: Suggestions| suggestions.set(value)),
        })
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
        Callback::from(move |value: String| new_name.set(value))
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
        Callback::from(move |_: MouseEvent| {
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

    let source_rows = avail_sources
        .iter()
        .map(|s| {
            let uri = s.uri.clone();
            let chosen = selected.contains(&uri);
            // Surface current ownership: adding a source already in another
            // project will move it here.
            let owner = s
                .project_id
                .as_ref()
                .and_then(|pid| projects.iter().find(|p| &p.id == pid))
                .map(|p| p.name.clone());
            let on_click = {
                let (toggle, uri) = (toggle_source.clone(), uri.clone());
                Callback::from(move |_: MouseEvent| toggle.emit(uri.clone()))
            };
            html! {
                <li class="choice-row" key={uri}>
                    <span class="choice-row__label" title={s.uri.clone()}>{ source_label(&s.uri) }</span>
                    if let Some(owner) = owner {
                        <Pill dot=false>{ format!("in {owner}") }</Pill>
                    }
                    <Button size={Size::Small} {on_click}
                        variant={if chosen { ButtonVariant::Subtle } else { ButtonVariant::Default }}>
                        { if chosen { "Remove" } else { "Add" } }
                    </Button>
                </li>
            }
        })
        .collect::<Html>();

    let drawer_footer = html! {
        <>
            <Button variant={ButtonVariant::Subtle} on_click={close_drawer.reform(|_: MouseEvent| ())}>
                { "Cancel" }
            </Button>
            <Button variant={ButtonVariant::Primary} on_click={on_create} loading={*submitting}>
                { "Create project" }
            </Button>
        </>
    };

    let bar = html! {
        <AppBar brand={brand()} actions={html! { <><ThemeToggle /><UserArea /></> }}>
            <QueryBar suggestions={(*suggestions).clone()} />
        </AppBar>
    };

    html! {
        <ContextProvider<ProjectsContext> context={projects_ctx}>
            <ContextProvider<SuggestionsContext> context={(*suggestions_ctx).clone()}>
                <Shell {bar} sidebar={html! { <Nav /> }} storage_key="analytics-sidebar" class="app-shell">
                    <div class="app-content">{ props.children.clone() }</div>
                    { footer() }
                </Shell>
            </ContextProvider<SuggestionsContext>>
            <Drawer open={*drawer_open} title="New project" on_close={close_drawer} footer={drawer_footer}>
                <div class="form">
                    if let Some(error) = &*new_error {
                        <Alert tone={Tone::Danger}>{ error.clone() }</Alert>
                    }
                    <Field label="Project name">
                        <TextInput placeholder="Defaults to the first source added"
                            value={(*new_name).clone()} on_input={on_name} />
                    </Field>
                    <Field label="Add sources" group=true
                        hint={avail_sources.is_empty().then_some(
                            "No reporting sources yet. They appear here once a site starts reporting."
                        )}>
                        <ul class="choice-rows">{ source_rows }</ul>
                    </Field>
                </div>
            </Drawer>
        </ContextProvider<ProjectsContext>>
    }
}

/// The signed-in user at the end of the app bar: who they are and the way out.
#[function_component(UserArea)]
fn user_area() -> Html {
    let auth = use_context::<AuthHandle>().expect("AuthHandle context");
    let Some(user) = auth.user.clone() else {
        return html! {};
    };
    html! {
        <UserMenu
            name={user.name.clone()}
            detail={user.email.clone().map(AttrValue::from)}
            show_name=true
            on_sign_out={auth.signout.clone()}
        />
    }
}

#[derive(Properties, PartialEq)]
struct QueryBarProps {
    suggestions: Suggestions,
}

/// The query bar: shows the active filter expression, and applies an edited
/// one on Enter (Escape reverts). Invalid syntax is flagged inline and never
/// navigates. Chips and click-to-filter edit the same expression — this is
/// just the power-user view of it.
#[function_component(QueryBar)]
fn query_bar(props: &QueryBarProps) -> Html {
    let filters = use_filters();
    let apply = use_apply_filters();
    let projects = use_context::<ProjectsContext>();
    let live = filters.query.to_expression();

    // The edit in progress, paired with the expression it started from: once
    // the applied expression moves on (a chip was removed, a row was clicked),
    // a draft based on the old one is stale and is dropped.
    let draft = use_state(|| None::<(String, String)>);
    let error = use_state(|| None::<String>);
    let editing = draft.as_ref().filter(|(base, _)| *base == live);
    let value = editing.map_or_else(|| live.clone(), |(_, text)| text.clone());

    let on_input = {
        let (draft, error, live) = (draft.clone(), error.clone(), live.clone());
        Callback::from(move |text: String| {
            error.set(None);
            draft.set(Some((live.clone(), text)));
        })
    };
    let on_submit = {
        let (draft, error) = (draft.clone(), error.clone());
        let (apply, filters) = (apply.clone(), filters.clone());
        Callback::from(move |text: String| {
            let expression = text.trim().to_string();
            if let Some(message) = validate_expression(&expression) {
                error.set(Some(message));
                return;
            }
            error.set(None);
            draft.set(None);
            apply.emit(filters.with_query(Query::parse(&expression)));
        })
    };
    let on_cancel = {
        let (draft, error) = (draft.clone(), error.clone());
        Callback::from(move |_| {
            error.set(None);
            draft.set(None);
        })
    };

    // Every dimension is a field; the values come from the routed page, with
    // the project list as the fallback for pages that publish none.
    let fields: Rc<[QueryField]> = Dim::ALL
        .into_iter()
        .map(|dim| {
            let published = props
                .suggestions
                .iter()
                .find(|(d, _)| *d == dim)
                .map(|(_, options)| options.iter().map(|o| o.value.clone()).collect::<Vec<_>>());
            let values = match (published, dim) {
                (Some(values), _) => values,
                (None, Dim::Project) => projects
                    .as_ref()
                    .map(|c| c.projects.iter().map(|p| p.name.clone()).collect())
                    .unwrap_or_default(),
                (None, _) => Vec::new(),
            };
            QueryField::new(dim.param())
                .description(dim.label())
                .values(values.into_iter().filter(|v| !v.is_empty()))
        })
        .collect();

    html! {
        <QueryInput
            class="app-query"
            {value}
            {on_input}
            {on_submit}
            {on_cancel}
            {fields}
            syntax={QuerySyntax::Expression}
            applied={AttrValue::from(live)}
            error={(*error).clone().map(AttrValue::from)}
            icon={icons::TERMINAL}
            mono=true
            label="Filter query"
            placeholder={r#"Filter, e.g. browser == "Chrome" && (country == "DE" || path like "/docs/*")"#}
        />
    }
}

/// The navigation sidebar: Dashboard, Exceptions, the project list (shown only
/// when projects exist), Tracking pixels, and Settings.
///
/// Dashboard/Exceptions links carry the current filter state along so switching
/// views never silently drops active filters. Project entries apply a project
/// filter on the dashboard rather than navigating to a separate page.
#[function_component(Nav)]
fn nav() -> Html {
    let projects = use_context::<ProjectsContext>()
        .map(|c| c.projects.clone())
        .unwrap_or_default();
    let route = use_route::<Route>();
    let filters = use_filters();

    let is_dashboard = matches!(route, Some(Route::Overview | Route::Project { .. }));
    let is_exceptions = matches!(route, Some(Route::Exceptions | Route::Exception { .. }));
    let active_project = filters.get(Dim::Project).map(str::to_lowercase);

    // Primary links keep the filter state; a page that can't honour a dimension
    // shows its chip as inert rather than dropping it.
    let with_filters =
        |route: Route, filters: &FilterSet| NavTarget::route(route).query(filters.to_pairs());

    let project_items = projects
        .iter()
        .map(|p| {
            // Filters address projects by name (matched case-insensitively,
            // like the server); ids still match for pre-rename links.
            let current = is_dashboard
                && active_project
                    .as_deref()
                    .is_some_and(|v| v == p.name.to_lowercase() || v == p.id);
            let href = with_filters(Route::Overview, &filters.with(Dim::Project, p.name.clone()));
            html! {
                <SidebarItem key={p.id.clone()} {href} {current} icon={icons::FOLDER}>
                    { &p.name }
                </SidebarItem>
            }
        })
        .collect::<Html>();

    html! {
        <Sidebar label="Analytics">
            <SidebarItem href={with_filters(Route::Overview, &filters)} icon={icons::LAYOUT_DASHBOARD}
                current={is_dashboard && active_project.is_none()}>
                { "Dashboard" }
            </SidebarItem>
            <SidebarItem href={with_filters(Route::Exceptions, &filters)} icon={icons::BUG}
                current={is_exceptions}>
                { "Exceptions" }
            </SidebarItem>
            if !projects.is_empty() {
                <SidebarGroup label="Projects">{ project_items }</SidebarGroup>
            }
            <SidebarGroup label="Manage">
                <SidebarItem href={NavTarget::route(Route::Pixels)} icon={icons::IMAGE}
                    current={matches!(route, Some(Route::Pixels))}>
                    { "Tracking pixels" }
                </SidebarItem>
                <SidebarItem href={NavTarget::route(Route::Settings)} icon={icons::SETTINGS}
                    current={matches!(route, Some(Route::Settings))}>
                    { "Settings" }
                </SidebarItem>
            </SidebarGroup>
        </Sidebar>
    }
}
