//! Shared rendering for a failed API call. A mid-session `401` is handled centrally
//! here: rather than leaving a dead-end "session expired" alert (with stale
//! navigation), it flips the auth state back to the sign-in screen.

use yew::prelude::*;

use crate::api::ApiError;
use crate::app::AuthHandle;

#[derive(Properties, PartialEq)]
pub struct ApiErrorAlertProps {
    pub error: ApiError,
}

#[function_component(ApiErrorAlert)]
pub fn api_error_alert(props: &ApiErrorAlertProps) -> Html {
    let auth = use_context::<AuthHandle>();
    let unauthorized = matches!(props.error, ApiError::Unauthorized);

    {
        let relogin = auth.as_ref().map(|a| a.relogin.clone());
        use_effect_with(unauthorized, move |&unauthorized| {
            if unauthorized {
                if let Some(relogin) = relogin {
                    relogin.emit(());
                }
            }
            || ()
        });
    }

    if unauthorized {
        html! { <p class="muted">{ "Your session expired. Returning to sign in…" }</p> }
    } else {
        html! { <div class="alert alert--error">{ props.error.to_string() }</div> }
    }
}
