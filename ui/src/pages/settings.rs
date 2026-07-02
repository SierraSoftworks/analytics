//! The Settings page: signed-in account, instance/runtime info (authenticated, so
//! it may reveal the version), reporting-source management, the tracker install
//! snippet, and a danger zone.

use analytics_api::{Instance, Source, SourceInput, SourceKind, source_label};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::api::{self, ApiError};
use crate::app::AuthHandle;
use crate::components::{
    ApiErrorAlert, Dropdown, DropdownItem, PageHeader, ProjectsContext, icons,
};

#[function_component(Settings)]
pub fn settings() -> Html {
    html! {
        <div class="page">
            <PageHeader title="Settings" subtitle="Your account, this instance, and onboarding." />
            <div class="settings">
                <AccountCard />
                <InstanceCard />
                <SourcesCard />
                <TrackerCard />
                <DangerZone />
            </div>
        </div>
    }
}

/// A human label for a source kind (avoids leaking Rust `Debug` formatting).
fn kind_label(kind: &SourceKind) -> &'static str {
    match kind {
        SourceKind::Website => "Website",
        SourceKind::Application => "Application",
    }
}

/// Every reporting source and its project assignment — the management home for
/// the dashboard's "unassigned sources" inbox.
#[function_component(SourcesCard)]
fn sources_card() -> Html {
    let sources = use_state(|| None::<Result<Vec<Source>, ApiError>>);
    let reload = use_state(|| 0u32);
    let projects = use_context::<ProjectsContext>()
        .map(|c| c.projects.clone())
        .unwrap_or_default();

    {
        let sources = sources.clone();
        use_effect_with(*reload, move |_| {
            spawn_local(async move {
                sources.set(Some(api::list_sources().await));
            });
            || ()
        });
    }

    let assign = {
        let reload = reload.clone();
        Callback::from(move |(uri, project_id): (String, String)| {
            let reload = reload.clone();
            spawn_local(async move {
                let input = SourceInput {
                    project_id: Some(project_id),
                    ..Default::default()
                };
                if api::update_source(&uri, &input).await.is_ok() {
                    reload.set(*reload + 1);
                }
            });
        })
    };

    let body = match &*sources {
        None => html! { <p class="muted">{ "Loading…" }</p> },
        Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
        Some(Ok(list)) if list.is_empty() => {
            html! { <p class="muted">{ "No sources have reported yet — they register automatically once a site or application starts reporting." }</p> }
        }
        Some(Ok(list)) => {
            let mut items: Vec<DropdownItem> = vec![DropdownItem::new("", "Unassigned")];
            items.extend(
                projects
                    .iter()
                    .map(|p| DropdownItem::new(p.id.clone(), p.name.clone())),
            );
            let rows = list
                .iter()
                .map(|s| {
                    let on_select = {
                        let (assign, uri) = (assign.clone(), s.uri.clone());
                        Callback::from(move |project_id: String| {
                            assign.emit((uri.clone(), project_id))
                        })
                    };
                    html! {
                        <tr key={s.uri.clone()}>
                            <td><code title={s.uri.clone()}>{ source_label(&s.uri) }</code></td>
                            <td>{ kind_label(&s.kind) }</td>
                            <td>
                                <Dropdown items={items.clone()}
                                    value={s.project_id.clone().unwrap_or_default()}
                                    placeholder="Unassigned" on_select={on_select} />
                            </td>
                        </tr>
                    }
                })
                .collect::<Html>();
            html! {
                <div class="card-table">
                    <table class="list">
                        <thead><tr><th>{ "Source" }</th><th>{ "Kind" }</th><th>{ "Project" }</th></tr></thead>
                        <tbody>{ rows }</tbody>
                    </table>
                </div>
            }
        }
    };

    html! {
        <div class="settings-card">
            <h2 class="settings-card__title">{ "Reporting sources" }</h2>
            <p class="settings-card__desc">{ "Every hostname and application that has reported events, and the project each belongs to." }</p>
            { body }
        </div>
    }
}

#[function_component(AccountCard)]
fn account_card() -> Html {
    let auth = use_context::<AuthHandle>();
    let body = match auth.as_ref().and_then(|a| a.user.clone()) {
        Some(user) => {
            let email = user
                .email
                .map(|e| html! { <><span class="kv__key">{ "Email" }</span><span class="kv__val">{ e }</span></> });
            html! {
                <div class="kv">
                    <span class="kv__key">{ "Name" }</span>
                    <span class="kv__val">{ user.name }</span>
                    { email }
                </div>
            }
        }
        None => {
            html! { <p class="muted">{ "Authentication is disabled on this server; no account is associated with this session." }</p> }
        }
    };
    html! {
        <div class="settings-card">
            <h2 class="settings-card__title">{ "Account" }</h2>
            { body }
        </div>
    }
}

#[function_component(InstanceCard)]
fn instance_card() -> Html {
    let instance = use_state(|| None::<Result<Instance, ApiError>>);
    {
        let instance = instance.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                instance.set(Some(api::instance().await));
            });
            || ()
        });
    }

    let body = match &*instance {
        None => html! { <p class="muted">{ "Loading…" }</p> },
        Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
        Some(Ok(i)) => {
            let rate = if i.rate_limiting {
                format!(
                    "{} req/min tracking · {} req/min unauthenticated",
                    i.tracking_per_minute, i.unauthenticated_per_minute
                )
            } else {
                "Disabled".to_string()
            };
            html! {
                <div class="kv">
                    <span class="kv__key">{ "Version" }</span>
                    <span class="kv__val">{ &i.version }</span>
                    <span class="kv__key">{ "Data retention" }</span>
                    <span class="kv__val">{ format!("{} days", i.retention_days) }</span>
                    <span class="kv__key">{ "Hot window" }</span>
                    <span class="kv__val">{ format!("{} hours", i.hot_window_hours) }</span>
                    <span class="kv__key">{ "Honour DNT / GPC" }</span>
                    <span class="kv__val">{ if i.honor_dnt { "Yes" } else { "No" } }</span>
                    <span class="kv__key">{ "Rate limiting" }</span>
                    <span class="kv__val">{ rate }</span>
                    <span class="kv__key">{ "Max auto-sources" }</span>
                    <span class="kv__val">{ i.max_auto_sources }</span>
                </div>
            }
        }
    };
    html! {
        <div class="settings-card">
            <h2 class="settings-card__title">{ "Instance" }</h2>
            <p class="settings-card__desc">{ "Runtime configuration of this analytics server." }</p>
            { body }
        </div>
    }
}

#[function_component(TrackerCard)]
fn tracker_card() -> Html {
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_else(|| "https://analytics.example.com".to_string());

    let snippet = format!(
        "<script\n  async\n  src=\"{origin}/tracker.js\"\n  data-api=\"{origin}\"\n  data-auto-capture-exceptions=\"true\"\n></script>"
    );

    // navigator.clipboard only exists in secure contexts; on a plain-HTTP LAN
    // deployment the getter is undefined and calling it throws, so the button
    // is only offered where it can work (the snippet is selectable regardless).
    let can_copy = web_sys::window().is_some_and(|w| w.is_secure_context());
    let on_copy = {
        let snippet = snippet.clone();
        Callback::from(move |_| {
            if let Some(win) = web_sys::window()
                && win.is_secure_context()
            {
                let _ = win.navigator().clipboard().write_text(&snippet);
            }
        })
    };

    html! {
        <div class="settings-card">
            <h2 class="settings-card__title">{ "Install the tracker" }</h2>
            <p class="settings-card__desc">
                { "Add this snippet to your site. Sources are identified by hostname — no per-site key to embed." }
            </p>
            <pre class="snippet">
                if can_copy {
                    <button class="btn btn--small snippet__copy" onclick={on_copy}>
                        <span class="menu__icon">{ icons::copy() }</span>{ "Copy" }
                    </button>
                }
                { snippet }
            </pre>
        </div>
    }
}

#[function_component(DangerZone)]
fn danger_zone() -> Html {
    let projects_ctx = use_context::<ProjectsContext>();
    let selected = use_state(String::new);
    let projects = projects_ctx
        .as_ref()
        .map(|c| c.projects.clone())
        .unwrap_or_default();

    let on_select = {
        let selected = selected.clone();
        Callback::from(move |value: String| selected.set(value))
    };
    let items: Vec<DropdownItem> = projects
        .iter()
        .map(|p| DropdownItem::new(p.id.clone(), p.name.clone()))
        .collect();

    let on_delete = {
        let selected = selected.clone();
        let reload = projects_ctx.as_ref().map(|c| c.reload.clone());
        Callback::from(move |_| {
            let id = (*selected).clone();
            if id.is_empty() {
                return;
            }
            let confirmed = web_sys::window()
                .and_then(|w| {
                    w.confirm_with_message(
                        "Delete this project? Its pixels are removed and its sources become unassigned. This cannot be undone.",
                    )
                    .ok()
                })
                .unwrap_or(false);
            if !confirmed {
                return;
            }
            let (selected, reload) = (selected.clone(), reload.clone());
            spawn_local(async move {
                if api::delete_project(&id).await.is_ok() {
                    selected.set(String::new());
                    if let Some(reload) = &reload {
                        reload.emit(());
                    }
                }
            });
        })
    };

    html! {
        <div class="settings-card settings-card--danger">
            <h2 class="settings-card__title">{ "Danger zone" }</h2>
            <p class="settings-card__desc">
                { "Deleting a project removes its tracking pixels and unassigns its sources. Historical events are retained under those sources." }
            </p>
            <div class="form-row">
                <Dropdown items={items} value={(*selected).clone()}
                    placeholder="Select a project…" on_select={on_select} />
                <button class="btn btn--danger" onclick={on_delete} disabled={(*selected).is_empty()}>
                    { "Delete project" }
                </button>
            </div>
        </div>
    }
}
