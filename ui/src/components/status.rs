//! Shared exception triage status rendering (used by the inbox and detail pages).

use analytics_api::ExceptionStatus;
use yew::prelude::*;

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
