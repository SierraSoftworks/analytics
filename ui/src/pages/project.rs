use analytics_api::{ExceptionGroup, ExceptionStatus, Pixel, PixelInput, Project as ProjectData, Stats};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api::{self, ApiError};
use crate::app::Route;
use crate::components::{Breakdown, MetricCards, TimeSeriesChart};

#[derive(Properties, PartialEq)]
pub struct ProjectProps {
    pub id: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Stats,
    Pixels,
    Exceptions,
}

#[function_component(Project)]
pub fn project(props: &ProjectProps) -> Html {
    let id = props.id.clone();
    let project = use_state(|| None::<Result<ProjectData, ApiError>>);
    let tab = use_state(|| Tab::Stats);

    {
        let project = project.clone();
        let id = id.clone();
        use_effect_with(id.clone(), move |_| {
            spawn_local(async move {
                project.set(Some(api::get_project(&id).await));
            });
            || ()
        });
    }

    let name = match &*project {
        Some(Ok(p)) => p.name.clone(),
        _ => "Project".to_string(),
    };

    let tab_button = |this: Tab, label: &str| {
        let tab = tab.clone();
        let active = *tab == this;
        let onclick = Callback::from(move |_| tab.set(this));
        html! {
            <button class={classes!("tab", active.then_some("tab--active"))} {onclick}>{ label }</button>
        }
    };

    html! {
        <div class="page">
            <p class="crumb"><Link<Route> to={Route::Overview}>{ "← Overview" }</Link<Route>></p>
            <h1>{ name }</h1>
            if let Some(Err(err)) = &*project {
                <div class="alert alert--error">{ err.to_string() }</div>
            }
            <div class="tabs">
                { tab_button(Tab::Stats, "Statistics") }
                { tab_button(Tab::Pixels, "Tracking pixels") }
                { tab_button(Tab::Exceptions, "Exceptions") }
            </div>
            {
                match *tab {
                    Tab::Stats => html! { <ProjectStats id={id.clone()} /> },
                    Tab::Pixels => html! { <ProjectPixels id={id.clone()} /> },
                    Tab::Exceptions => html! { <ProjectExceptions id={id.clone()} /> },
                }
            }
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct IdProps {
    id: String,
}

#[function_component(ProjectStats)]
fn project_stats(props: &IdProps) -> Html {
    let id = props.id.clone();
    let stats = use_state(|| None::<Result<Stats, ApiError>>);
    {
        let stats = stats.clone();
        use_effect_with(id.clone(), move |id| {
            let id = id.clone();
            spawn_local(async move {
                stats.set(Some(api::project_stats(&id, "interval=day").await));
            });
            || ()
        });
    }

    match &*stats {
        None => html! { <p class="muted">{ "Loading…" }</p> },
        Some(Err(err)) => html! { <div class="alert alert--error">{ err.to_string() }</div> },
        Some(Ok(s)) => html! {
            <>
                <MetricCards summary={s.summary.clone()} />
                <div class="panel"><TimeSeriesChart points={s.timeseries.clone()} /></div>
                <div class="breakdowns">
                    <Breakdown title="Top pages" rows={s.pages.clone()} />
                    <Breakdown title="Referrers" rows={s.referrers.clone()} />
                    <Breakdown title="Sources" rows={s.sources.clone()} />
                    <Breakdown title="Browsers" rows={s.browsers.clone()} />
                    <Breakdown title="Operating systems" rows={s.operating_systems.clone()} />
                    <Breakdown title="Devices" rows={s.devices.clone()} />
                    <Breakdown title="Countries" rows={s.countries.clone()} />
                    <Breakdown title="Languages" rows={s.languages.clone()} />
                </div>
            </>
        },
    }
}

#[function_component(ProjectPixels)]
fn project_pixels(props: &IdProps) -> Html {
    let id = props.id.clone();
    let pixels = use_state(|| None::<Result<Vec<Pixel>, ApiError>>);
    let reload = use_state(|| 0u32);
    let name = use_state(String::new);
    let event = use_state(String::new);

    {
        let pixels = pixels.clone();
        let id = id.clone();
        use_effect_with((id.clone(), *reload), move |(id, _)| {
            let id = id.clone();
            spawn_local(async move {
                pixels.set(Some(api::list_pixels(&id).await));
            });
            || ()
        });
    }

    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();

    let on_name = bind_input(name.clone());
    let on_event = bind_input(event.clone());

    let on_create = {
        let (id, name, event, reload) = (id.clone(), name.clone(), event.clone(), reload.clone());
        Callback::from(move |_| {
            let title = (*name).trim().to_string();
            if title.is_empty() {
                return;
            }
            let input = PixelInput {
                name: title,
                event_name: Some((*event).trim().to_string()).filter(|e| !e.is_empty()),
                metadata: Default::default(),
            };
            let (id, name, event, reload) = (id.clone(), name.clone(), event.clone(), reload.clone());
            spawn_local(async move {
                if api::create_pixel(&id, &input).await.is_ok() {
                    name.set(String::new());
                    event.set(String::new());
                    reload.set(*reload + 1);
                }
            });
        })
    };

    html! {
        <>
            <div class="form-row">
                <input class="input" placeholder="Pixel name (e.g. June newsletter)" value={(*name).clone()} oninput={on_name} />
                <input class="input" placeholder="Event name (default: pixel)" value={(*event).clone()} oninput={on_event} />
                <button class="btn btn--primary" onclick={on_create}>{ "Create pixel" }</button>
            </div>
            {
                match &*pixels {
                    None => html! { <p class="muted">{ "Loading…" }</p> },
                    Some(Err(err)) => html! { <div class="alert alert--error">{ err.to_string() }</div> },
                    Some(Ok(list)) if list.is_empty() => html! { <p class="muted">{ "No pixels yet." }</p> },
                    Some(Ok(list)) => html! {
                        <table class="list">
                            <thead><tr><th>{ "Name" }</th><th>{ "Event" }</th><th>{ "Embed URL" }</th><th></th></tr></thead>
                            <tbody>
                            { for list.iter().map(|p| {
                                let url = format!("{origin}/track/gif/{}.gif", p.id);
                                let on_delete = {
                                    let (pid, reload) = (p.id.clone(), reload.clone());
                                    Callback::from(move |_| {
                                        let (pid, reload) = (pid.clone(), reload.clone());
                                        spawn_local(async move {
                                            if api::delete_pixel(&pid).await.is_ok() {
                                                reload.set(*reload + 1);
                                            }
                                        });
                                    })
                                };
                                html! {
                                    <tr>
                                        <td>{ &p.name }</td>
                                        <td>{ &p.event_name }</td>
                                        <td><code class="embed">{ url }</code></td>
                                        <td><button class="btn btn--ghost" onclick={on_delete}>{ "Delete" }</button></td>
                                    </tr>
                                }
                            }) }
                            </tbody>
                        </table>
                    },
                }
            }
        </>
    }
}

#[function_component(ProjectExceptions)]
fn project_exceptions(props: &IdProps) -> Html {
    let id = props.id.clone();
    let groups = use_state(|| None::<Result<Vec<ExceptionGroup>, ApiError>>);
    {
        let groups = groups.clone();
        use_effect_with(id.clone(), move |id| {
            let id = id.clone();
            spawn_local(async move {
                groups.set(Some(api::list_exceptions(&id, "interval=day").await));
            });
            || ()
        });
    }

    match &*groups {
        None => html! { <p class="muted">{ "Loading…" }</p> },
        Some(Err(err)) => html! { <div class="alert alert--error">{ err.to_string() }</div> },
        Some(Ok(list)) if list.is_empty() => html! { <p class="muted">{ "No exceptions reported." }</p> },
        Some(Ok(list)) => html! {
            <table class="list">
                <thead><tr><th>{ "Type" }</th><th>{ "Message" }</th><th>{ "Count" }</th><th>{ "Status" }</th></tr></thead>
                <tbody>
                { for list.iter().map(|g| html! {
                    <tr>
                        <td>
                            <Link<Route> to={Route::Exception { project: id.clone(), group: g.group_id.clone() }}>
                                { &g.exc_type }
                            </Link<Route>>
                        </td>
                        <td class="ellipsis" title={g.sample_message.clone()}>{ &g.sample_message }</td>
                        <td>{ g.count }</td>
                        <td><span class={status_class(g.status)}>{ status_label(g.status) }</span></td>
                    </tr>
                }) }
                </tbody>
            </table>
        },
    }
}

fn bind_input(state: UseStateHandle<String>) -> Callback<InputEvent> {
    Callback::from(move |e: InputEvent| {
        let input: HtmlInputElement = e.target_unchecked_into();
        state.set(input.value());
    })
}

pub fn status_label(status: ExceptionStatus) -> &'static str {
    match status {
        ExceptionStatus::Unresolved => "Unresolved",
        ExceptionStatus::Resolved => "Resolved",
        ExceptionStatus::Ignored => "Ignored",
    }
}

pub fn status_class(status: ExceptionStatus) -> Classes {
    let modifier = match status {
        ExceptionStatus::Unresolved => "badge--warn",
        ExceptionStatus::Resolved => "badge--ok",
        ExceptionStatus::Ignored => "badge--muted",
    };
    classes!("badge", modifier)
}
