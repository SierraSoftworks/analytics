//! The Settings page: signed-in account, instance/runtime info (authenticated, so
//! it may reveal the version), reporting-source management, the tracker install
//! snippet, and a danger zone.

use analytics_api::{Instance, Source, SourceInput, SourceKind, source_label};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yewi::prelude::{
    Button, ButtonVariant, Card, CodeBlock, ConfirmDialog, DescriptionItem, DescriptionList,
    InlineCode, PageHeader, Select, SelectOption, SimpleTable, Size, Text, TextColor, TextTag,
    attrs, icons,
};

use crate::api::{self, ApiError};
use crate::app::AuthHandle;
use crate::components::page::{Section, loading};
use crate::components::{ApiErrorAlert, ProjectDrawer, ProjectsContext};

/// The option standing for "no project" in the per-source project picker.
const UNASSIGNED: &str = "";

#[function_component(Settings)]
pub fn settings() -> Html {
    html! {
        <div class="page page--narrow">
            <PageHeader title="Settings" />
            <AccountSection />
            <InstanceSection />
            <ProjectsSection />
            <SourcesSection />
            <TrackerSection />
            <DangerZone />
        </div>
    }
}

fn muted(text: &'static str) -> Html {
    html! { <Text tag={TextTag::P} color={TextColor::Subtle}>{ text }</Text> }
}

/// Project management: the project list with per-project management (rename,
/// membership, delete via the shared drawer) and project creation.
#[function_component(ProjectsSection)]
fn projects_section() -> Html {
    let projects_ctx = use_context::<ProjectsContext>();
    let manage = use_state(|| None::<String>);
    let projects = projects_ctx
        .as_ref()
        .map(|c| c.projects.clone())
        .unwrap_or_default();

    let on_new = {
        let open_new = projects_ctx.as_ref().map(|c| c.open_new.clone());
        Callback::from(move |_: MouseEvent| {
            if let Some(open_new) = &open_new {
                open_new.emit(());
            }
        })
    };
    let close_manage = {
        let manage = manage.clone();
        Callback::from(move |_: ()| manage.set(None))
    };
    let on_changed = {
        let reload = projects_ctx.as_ref().map(|c| c.reload.clone());
        Callback::from(move |_: ()| {
            if let Some(reload) = &reload {
                reload.emit(());
            }
        })
    };

    let body = if projects.is_empty() {
        muted("No projects yet. Create one to group your reporting sources.")
    } else {
        let rows = projects
            .iter()
            .map(|p| {
                let on_click = {
                    let (manage, id) = (manage.clone(), p.id.clone());
                    Callback::from(move |_: MouseEvent| manage.set(Some(id.clone())))
                };
                html! {
                    <li class="choice-row" key={p.id.clone()}>
                        <span class="choice-row__label">{ &p.name }</span>
                        <Button size={Size::Small} variant={ButtonVariant::Subtle} icon={icons::SETTINGS} {on_click}>
                            { "Manage" }
                        </Button>
                    </li>
                }
            })
            .collect::<Html>();
        html! { <ul class="choice-rows">{ rows }</ul> }
    };

    let actions = html! {
        <Button variant={ButtonVariant::Primary} size={Size::Small} icon={icons::PLUS} on_click={on_new}>
            { "New project" }
        </Button>
    };

    html! {
        <Section title="Projects" {actions}
            description="Group reporting sources into projects to view them in aggregate.">
            { body }
            <ProjectDrawer project_id={(*manage).clone()} on_close={close_manage} on_changed={on_changed} />
        </Section>
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
#[function_component(SourcesSection)]
fn sources_section() -> Html {
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
        None => loading(),
        Some(Err(err)) => html! { <ApiErrorAlert error={err.clone()} /> },
        Some(Ok(list)) if list.is_empty() => muted(
            "No sources have reported yet. They register automatically once a site or application starts reporting.",
        ),
        Some(Ok(list)) => {
            let mut options = vec![SelectOption::new(UNASSIGNED, "Unassigned")];
            options.extend(
                projects
                    .iter()
                    .map(|p| SelectOption::new(p.id.clone(), p.name.clone())),
            );
            let rows = list
                .iter()
                .map(|s| {
                    let on_change = {
                        let (assign, uri) = (assign.clone(), s.uri.clone());
                        Callback::from(move |project_id: AttrValue| {
                            assign.emit((uri.clone(), project_id.to_string()))
                        })
                    };
                    html! {
                        <tr key={s.uri.clone()}>
                            <th scope="row">
                                <InlineCode attrs={attrs! { "title" => s.uri.clone() }}>{ source_label(&s.uri) }</InlineCode>
                            </th>
                            <td>{ kind_label(&s.kind) }</td>
                            <td>
                                <Select options={options.clone()} size={Size::Small} {on_change}
                                    value={AttrValue::from(s.project_id.clone().unwrap_or_default())}
                                    label={format!("Project for {}", source_label(&s.uri))} />
                            </td>
                        </tr>
                    }
                })
                .collect::<Html>();
            html! {
                <SimpleTable label="Reporting sources">
                    <thead>
                        <tr>
                            <th scope="col">{ "Source" }</th>
                            <th scope="col">{ "Kind" }</th>
                            <th scope="col">{ "Project" }</th>
                        </tr>
                    </thead>
                    <tbody>{ rows }</tbody>
                </SimpleTable>
            }
        }
    };

    html! {
        <Section title="Reporting sources"
            description="Every hostname and application that has reported events, and the project each belongs to.">
            { body }
        </Section>
    }
}

#[function_component(AccountSection)]
fn account_section() -> Html {
    let auth = use_context::<AuthHandle>();
    let body = match auth.as_ref().and_then(|a| a.user.clone()) {
        Some(user) => html! {
            <DescriptionList>
                <DescriptionItem term="Name">{ user.name }</DescriptionItem>
                if let Some(email) = user.email {
                    <DescriptionItem term="Email">{ email }</DescriptionItem>
                }
            </DescriptionList>
        },
        None => muted(
            "Authentication is disabled on this server; no account is associated with this session.",
        ),
    };
    html! { <Section title="Account">{ body }</Section> }
}

#[function_component(InstanceSection)]
fn instance_section() -> Html {
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
        None => loading(),
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
                <DescriptionList>
                    <DescriptionItem term="Version" mono=true>{ &i.version }</DescriptionItem>
                    <DescriptionItem term="Data retention">{ format!("{} days", i.retention_days) }</DescriptionItem>
                    <DescriptionItem term="Hot window">{ format!("{} hours", i.hot_window_hours) }</DescriptionItem>
                    <DescriptionItem term="Honour DNT / GPC">{ if i.honor_dnt { "Yes" } else { "No" } }</DescriptionItem>
                    <DescriptionItem term="Rate limiting">{ rate }</DescriptionItem>
                    <DescriptionItem term="Max auto-sources">{ i.max_auto_sources }</DescriptionItem>
                </DescriptionList>
            }
        }
    };
    html! {
        <Section title="Instance" description="Runtime configuration of this analytics server.">
            { body }
        </Section>
    }
}

#[function_component(TrackerSection)]
fn tracker_section() -> Html {
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_else(|| "https://analytics.example.com".to_string());

    let snippet = format!(
        "<script\n  async\n  src=\"{origin}/tracker.js\"\n  data-auto-capture-exceptions=\"true\"\n></script>"
    );

    // navigator.clipboard only exists in secure contexts; on a plain-HTTP LAN
    // deployment the copy button could not work, so it is only offered where
    // it can (the snippet is selectable regardless).
    let can_copy = web_sys::window().is_some_and(|w| w.is_secure_context());

    html! {
        <Section title="Install the tracker"
            description="Add this snippet to your site. Sources are identified by hostname, so there is no per-site key to embed.">
            <CodeBlock code={snippet} language="html" copy={can_copy} copy_label="Copy snippet" />
        </Section>
    }
}

/// Deleting a project is the one thing here that cannot be taken back, so it
/// is set apart from the settings around it on a surface of its own.
#[function_component(DangerZone)]
fn danger_zone() -> Html {
    let projects_ctx = use_context::<ProjectsContext>();
    let selected = use_state(|| None::<AttrValue>);
    let confirming = use_state(|| false);
    let projects = projects_ctx
        .as_ref()
        .map(|c| c.projects.clone())
        .unwrap_or_default();

    let on_change = {
        let selected = selected.clone();
        Callback::from(move |value: AttrValue| selected.set(Some(value)))
    };
    let options: Vec<SelectOption> = projects
        .iter()
        .map(|p| SelectOption::new(p.id.clone(), p.name.clone()))
        .collect();
    let name = selected
        .as_ref()
        .and_then(|id| projects.iter().find(|p| p.id == id.as_str()))
        .map(|p| p.name.clone());

    let ask = {
        let confirming = confirming.clone();
        Callback::from(move |_: MouseEvent| confirming.set(true))
    };
    let on_cancel = {
        let confirming = confirming.clone();
        Callback::from(move |_: ()| confirming.set(false))
    };
    let on_confirm = {
        let (selected, confirming) = (selected.clone(), confirming.clone());
        let reload = projects_ctx.as_ref().map(|c| c.reload.clone());
        Callback::from(move |_: ()| {
            confirming.set(false);
            let Some(id) = (*selected).clone() else {
                return;
            };
            let (selected, reload) = (selected.clone(), reload.clone());
            spawn_local(async move {
                if api::delete_project(&id).await.is_ok() {
                    selected.set(None);
                    if let Some(reload) = &reload {
                        reload.emit(());
                    }
                }
            });
        })
    };

    html! {
        <Card class="danger-zone" title="Danger zone"
            subtitle="Deleting a project removes its tracking pixels and unassigns its sources. Historical events are retained under those sources.">
            <div class="form__row">
                <Select {options} value={(*selected).clone()} placeholder="Select a project…"
                    {on_change} label="Project to delete" fit=true />
                <Button variant={ButtonVariant::Danger} on_click={ask} disabled={selected.is_none()}>
                    { "Delete project" }
                </Button>
            </div>
            <ConfirmDialog open={*confirming} danger=true confirm_label="Delete project"
                title={format!("Delete {}?", name.as_deref().unwrap_or("this project"))}
                {on_confirm} {on_cancel}>
                { "Its pixels are removed and its sources become unassigned. This cannot be undone." }
            </ConfirmDialog>
        </Card>
    }
}
