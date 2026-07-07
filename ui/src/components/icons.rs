//! Small inline SVG glyphs shared across the chrome. Each returns a stroked,
//! `currentColor` icon so it inherits the surrounding text colour.

use yew::prelude::*;

fn icon(children: Html) -> Html {
    html! {
        <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor"
            stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            { children }
        </svg>
    }
}

pub fn overview() -> Html {
    icon(html! {
        <>
            <rect x="3" y="3" width="7" height="9" rx="1" />
            <rect x="14" y="3" width="7" height="5" rx="1" />
            <rect x="14" y="12" width="7" height="9" rx="1" />
            <rect x="3" y="16" width="7" height="5" rx="1" />
        </>
    })
}

pub fn exceptions() -> Html {
    icon(html! {
        <>
            <path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
            <line x1="12" y1="9" x2="12" y2="13" />
            <line x1="12" y1="17" x2="12.01" y2="17" />
        </>
    })
}

pub fn pixels() -> Html {
    icon(html! {
        <>
            <rect x="3" y="3" width="18" height="18" rx="2" />
            <circle cx="8.5" cy="8.5" r="1.5" />
            <path d="m21 15-5-5L5 21" />
        </>
    })
}

pub fn settings() -> Html {
    icon(html! {
        <>
            <circle cx="12" cy="12" r="3" />
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </>
    })
}

pub fn copy() -> Html {
    icon(html! {
        <>
            <rect x="9" y="9" width="13" height="13" rx="2" />
            <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
        </>
    })
}

pub fn filter() -> Html {
    icon(html! {
        <polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3" />
    })
}

pub fn close() -> Html {
    icon(html! {
        <>
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
        </>
    })
}

pub fn plus() -> Html {
    icon(html! {
        <>
            <line x1="12" y1="5" x2="12" y2="19" />
            <line x1="5" y1="12" x2="19" y2="12" />
        </>
    })
}

pub fn gear() -> Html {
    icon(html! {
        <>
            <circle cx="12" cy="12" r="3" />
            <path d="M12 2v3m0 14v3M2 12h3m14 0h3M4.9 4.9l2.1 2.1m10 10 2.1 2.1m0-14.2-2.1 2.1m-10 10-2.1 2.1" />
        </>
    })
}

pub fn menu() -> Html {
    icon(html! {
        <>
            <line x1="4" y1="7" x2="20" y2="7" />
            <line x1="4" y1="12" x2="20" y2="12" />
            <line x1="4" y1="17" x2="20" y2="17" />
        </>
    })
}

/// A browser client (globe), for session traces reported from a website.
pub fn globe() -> Html {
    icon(html! {
        <>
            <circle cx="12" cy="12" r="10" />
            <line x1="2" y1="12" x2="22" y2="12" />
            <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
        </>
    })
}

/// An application client (terminal window), for session traces reported by an app.
pub fn terminal() -> Html {
    icon(html! {
        <>
            <rect x="2" y="4" width="20" height="16" rx="2" />
            <polyline points="6 9 9 12 6 15" />
            <line x1="12" y1="15" x2="17" y2="15" />
        </>
    })
}

pub fn chevron_right() -> Html {
    icon(html! {
        <polyline points="9 18 15 12 9 6" />
    })
}

/// A checkmark (the resolve action).
pub fn check() -> Html {
    icon(html! {
        <polyline points="20 6 9 17 4 12" />
    })
}

/// A muted bell (the ignore action).
pub fn mute() -> Html {
    icon(html! {
        <>
            <path d="M8.7 3a6 6 0 0 1 9.3 5c0 3.2.6 5.3 1.3 6.7" />
            <path d="M17 17H3s3-2 3-9a6 6 0 0 1 .3-1.9" />
            <path d="M10.3 21a1.94 1.94 0 0 0 3.4 0" />
            <line x1="2" y1="2" x2="22" y2="22" />
        </>
    })
}

/// A checkmark struck through (the reopen action: un-resolve). The strike
/// carries its own class so the button can colour it independently of the
/// checkmark.
pub fn check_struck() -> Html {
    icon(html! {
        <>
            <polyline points="20 6 9 17 4 12" />
            <line class="icon-strike" x1="4" y1="5" x2="20" y2="21" />
        </>
    })
}

pub fn chevron_left() -> Html {
    icon(html! {
        <polyline points="15 18 9 12 15 6" />
    })
}
