//! A right-side slide-in drawer used for all create/edit wizards. It dims the page
//! behind a scrim (click to dismiss), traps initial focus, and closes on Escape.

use web_sys::HtmlElement;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct DrawerProps {
    pub open: bool,
    pub title: AttrValue,
    pub on_close: Callback<()>,
    /// The form/body content.
    #[prop_or_default]
    pub children: Html,
    /// Footer actions (e.g. Cancel / Save), pinned to the bottom.
    #[prop_or_default]
    pub footer: Html,
}

#[function_component(Drawer)]
pub fn drawer(props: &DrawerProps) -> Html {
    let panel = use_node_ref();

    // Move focus into the drawer when it opens, so Escape (handled on the panel)
    // works immediately and keyboard users land inside the dialog.
    {
        let panel = panel.clone();
        use_effect_with(props.open, move |&open| {
            if open && let Some(el) = panel.cast::<HtmlElement>() {
                let _ = el.focus();
            }
            || ()
        });
    }

    if !props.open {
        return html! {};
    }

    let close = {
        let on_close = props.on_close.clone();
        Callback::from(move |_| on_close.emit(()))
    };
    let on_keydown = {
        let on_close = props.on_close.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Escape" {
                on_close.emit(());
            }
        })
    };
    // Swallow clicks inside the panel so they don't reach the scrim.
    let stop = Callback::from(|e: MouseEvent| e.stop_propagation());

    let footer = (props.footer != Html::default())
        .then(|| html! { <footer class="drawer__footer">{ props.footer.clone() }</footer> });

    html! {
        <div class="drawer-root">
            <div class="drawer__scrim" onclick={close.clone()} />
            <aside
                ref={panel}
                class="drawer"
                role="dialog"
                aria-modal="true"
                aria-label={props.title.clone()}
                tabindex="-1"
                onkeydown={on_keydown}
                onclick={stop}
            >
                <header class="drawer__head">
                    <h2 class="drawer__title">{ props.title.clone() }</h2>
                    <button class="drawer__close" aria-label="Close" onclick={close}>
                        <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor"
                            stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                            <line x1="18" y1="6" x2="6" y2="18" />
                            <line x1="6" y1="6" x2="18" y2="18" />
                        </svg>
                    </button>
                </header>
                <div class="drawer__body">{ props.children.clone() }</div>
                { footer }
            </aside>
        </div>
    }
}
