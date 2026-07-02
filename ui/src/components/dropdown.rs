//! A cohesively-themed dropdown (custom `<select>` replacement) so menus match the
//! dark theme rather than rendering the OS-native control. The menu is positioned
//! `fixed` against the trigger's measured rect so it is never clipped by a scrolling
//! ancestor (e.g. a drawer body). It closes on selection, Escape, or a click outside.

use web_sys::HtmlElement;
use yew::prelude::*;

#[derive(Clone, PartialEq)]
pub struct DropdownItem {
    pub value: AttrValue,
    pub label: AttrValue,
}

impl DropdownItem {
    pub fn new(value: impl Into<AttrValue>, label: impl Into<AttrValue>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct DropdownProps {
    pub items: Vec<DropdownItem>,
    /// The currently-selected value (matched against `items[].value`).
    #[prop_or_default]
    pub value: AttrValue,
    pub on_select: Callback<String>,
    /// Shown on the trigger when nothing matches `value`.
    #[prop_or_default]
    pub placeholder: AttrValue,
    /// Stretch to fill the container (for use inside forms/drawers).
    #[prop_or(false)]
    pub block: bool,
    #[prop_or(false)]
    pub disabled: bool,
}

#[function_component(Dropdown)]
pub fn dropdown(props: &DropdownProps) -> Html {
    let open = use_state(|| false);
    // `(top, left, width)` of the menu, measured from the trigger when opening.
    let anchor = use_state(|| (0.0f64, 0.0f64, 0.0f64));
    let trigger = use_node_ref();

    let current = props
        .items
        .iter()
        .find(|i| i.value == props.value)
        .map(|i| i.label.clone());

    let toggle = {
        let (open, anchor, trigger) = (open.clone(), anchor.clone(), trigger.clone());
        Callback::from(move |_: MouseEvent| {
            if *open {
                open.set(false);
            } else {
                if let Some(el) = trigger.cast::<HtmlElement>() {
                    let r = el.get_bounding_client_rect();
                    anchor.set((r.bottom() + 4.0, r.left(), r.width()));
                }
                open.set(true);
            }
        })
    };
    let close = {
        let open = open.clone();
        Callback::from(move |_: MouseEvent| open.set(false))
    };
    // Escape collapses the menu (and stops the event so an enclosing drawer's own
    // Escape-to-close doesn't also fire); when closed, the event bubbles normally.
    let on_keydown = {
        let open = open.clone();
        Callback::from(move |e: KeyboardEvent| {
            if *open && e.key() == "Escape" {
                e.stop_propagation();
                open.set(false);
            }
        })
    };

    let menu = open.then(|| {
        let (top, left, width) = *anchor;
        let style = format!("top:{top}px; left:{left}px; min-width:{width}px;");
        let options = props.items.iter().map(|item| {
            let selected = item.value == props.value;
            let onclick = {
                let (on_select, open, value) = (props.on_select.clone(), open.clone(), item.value.to_string());
                Callback::from(move |_: MouseEvent| {
                    on_select.emit(value.clone());
                    open.set(false);
                })
            };
            let class = classes!("dropdown__option", selected.then_some("dropdown__option--selected"));
            html! {
                <li class={class} role="option" aria-selected={selected.to_string()} onclick={onclick}>
                    <span>{ item.label.clone() }</span>
                    if selected {
                        <svg class="dropdown__check" viewBox="0 0 24 24" width="14" height="14" fill="none"
                            stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
                            <polyline points="20 6 9 17 4 12" />
                        </svg>
                    }
                </li>
            }
        }).collect::<Html>();
        html! {
            <>
                <div class="dropdown__backdrop" onclick={close} />
                <ul class="dropdown__menu" role="listbox" style={style}>{ options }</ul>
            </>
        }
    });

    let label = current.unwrap_or_else(|| props.placeholder.clone());
    let class = classes!("dropdown", props.block.then_some("dropdown--block"));

    html! {
        <div class={class} onkeydown={on_keydown}>
            <button
                ref={trigger}
                type="button"
                class="dropdown__trigger"
                onclick={toggle}
                disabled={props.disabled}
                aria-haspopup="listbox"
                aria-expanded={open.to_string()}
            >
                <span class="dropdown__label">{ label }</span>
                <svg class="dropdown__caret" viewBox="0 0 24 24" width="15" height="15" fill="none"
                    stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <polyline points="6 9 12 15 18 9" />
                </svg>
            </button>
            { menu }
        </div>
    }
}
