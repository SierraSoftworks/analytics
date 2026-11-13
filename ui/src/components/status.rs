//! Shared exception triage status rendering (used by the inbox and detail pages).

use analytics_api::ExceptionStatus;
use yew::prelude::*;
use yewi::prelude::{Pill, Tone};

pub fn status_label(status: ExceptionStatus) -> &'static str {
    match status {
        ExceptionStatus::Unresolved => "Unresolved",
        ExceptionStatus::Resolved => "Resolved",
        ExceptionStatus::Ignored => "Muted",
    }
}

/// The triage status as a pill: unresolved asks for attention, resolved is
/// settled, muted recedes.
pub fn status_pill(status: ExceptionStatus) -> Html {
    let tone = match status {
        ExceptionStatus::Unresolved => Tone::Warn,
        ExceptionStatus::Resolved => Tone::Ok,
        ExceptionStatus::Ignored => Tone::Neutral,
    };
    html! { <Pill {tone}>{ status_label(status) }</Pill> }
}
