//! The Settings page: signed-in account, instance/runtime info (authenticated, so
//! it may reveal the version), the tracker install snippet, and a danger zone.

use analytics_api::Instance;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::api::{self, ApiError};
use crate::app::AuthHandle;
use crate::components::{ApiErrorAlert, Dropdown, DropdownItem, PageHeader, ProjectsContext, icons};

#[function_component(Settings)]
pub fn settings() -> Html {
    html! {
        <div class="page">
            <PageHeader title="Settings" subtitle="Your account, this instance, and onboarding." />
            <div class="settings">
                <AccountCard />
                <InstanceCard />
                <TrackerCard />
                <DangerZone />
            </div>
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
                format!("{} req/min tracking · {} req/min unauthenticated", i.tracking_per_minute, i.unauthenticated_per_minute)
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

    let on_copy = {
        let snippet = snippet.clone();
        Callback::from(move |_| {
            if let Some(win) = web_sys::window() {
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
                <button class="btn btn--small snippet__copy" onclick={on_copy}>
                    <span class="menu__icon">{ icons::copy() }</span>{ "Copy" }
                </button>
                { snippet }
            </pre>
        </div>
    }
}

#[function_component(DangerZone)]
fn danger_zone() -> Html {
    let projects_ctx = use_context::<ProjectsContext>();
    let selected = use_state(String::new);
    let projects = projects_ctx.as_ref().map(|c| c.projects.clone()).unwrap_or_default();

    let on_select = {
        let selected = selected.clone();
        Callback::from(move |value: String| selected.set(value))
    };
    let items: Vec<DropdownItem> =
        projects.iter().map(|p| DropdownItem::new(p.id.clone(), p.name.clone())).collect();

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
