use yew::prelude::*;

/// The severity of an [`Alert`], selecting its colour and icon.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // The full set is part of the component's API.
pub enum AlertKind {
    Error,
    Warning,
    Info,
    Success,
}

impl AlertKind {
    fn class(self) -> &'static str {
        match self {
            AlertKind::Error => "alert--error",
            AlertKind::Warning => "alert--warning",
            AlertKind::Info => "alert--info",
            AlertKind::Success => "alert--success",
        }
    }

    fn icon(self) -> Html {
        let path = match self {
            AlertKind::Error => html! {
                <>
                    <circle cx="12" cy="12" r="10" />
                    <line x1="12" y1="8" x2="12" y2="12" />
                    <line x1="12" y1="16" x2="12.01" y2="16" />
                </>
            },
            AlertKind::Warning => html! {
                <>
                    <path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                    <line x1="12" y1="9" x2="12" y2="13" />
                    <line x1="12" y1="17" x2="12.01" y2="17" />
                </>
            },
            AlertKind::Info => html! {
                <>
                    <circle cx="12" cy="12" r="10" />
                    <line x1="12" y1="16" x2="12" y2="12" />
                    <line x1="12" y1="8" x2="12.01" y2="8" />
                </>
            },
            AlertKind::Success => html! {
                <>
                    <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" />
                    <polyline points="22 4 12 14.01 9 11.01" />
                </>
            },
        };
        html! {
            <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor"
                stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                { path }
            </svg>
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct AlertProps {
    pub kind: AlertKind,
    pub title: AttrValue,
    #[prop_or_default]
    pub message: Option<AttrValue>,
    /// Recovery actions (buttons) rendered in the alert's action row.
    #[prop_or_default]
    pub children: Html,
}

/// A page-level notice used to surface errors and offer a way to recover.
#[function_component(Alert)]
pub fn alert(props: &AlertProps) -> Html {
    let message = props
        .message
        .as_ref()
        .map(|m| html! { <p class="alert__message">{ m.clone() }</p> });

    let actions = (props.children != Html::default())
        .then(|| html! { <div class="alert__actions">{ props.children.clone() }</div> });

    html! {
        <div class={classes!("alert", props.kind.class())} role="alert">
            <span class="alert__icon">{ props.kind.icon() }</span>
            <div class="alert__body">
                <span class="alert__title">{ props.title.clone() }</span>
                { message }
                { actions }
            </div>
        </div>
    }
}
