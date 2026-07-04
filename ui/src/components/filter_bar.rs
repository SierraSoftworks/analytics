//! The persistent filter bar: active filter chips (removable, one per
//! dimension), the time-range picker, and the add-filter popover. All state
//! lives in the URL via [`crate::filters`]; this component only renders it and
//! pushes changes.

use std::rc::Rc;

use web_sys::HtmlInputElement;
use yew::prelude::*;

use crate::components::icons;
use crate::components::{Dropdown, DropdownItem, ProjectsContext};
use crate::filters::{Dim, FilterSet, RangePreset, TimeRange, use_apply_filters, use_filters};
use crate::format::{country_flag, country_name, language_name, short_date};

/// One suggested value for the add-filter popover.
#[derive(Clone, PartialEq)]
pub struct SuggestOption {
    pub value: String,
    pub label: String,
}

#[derive(Properties, PartialEq)]
pub struct FilterBarProps {
    /// Per-dimension value suggestions (typically the current breakdown rows).
    #[prop_or_default]
    pub suggestions: Rc<Vec<(Dim, Vec<SuggestOption>)>>,
    /// Exceptions mode: dimensions exception events don't carry render as
    /// inert chips ("not applied here") and are hidden from the add menu.
    #[prop_or(false)]
    pub restricted: bool,
}

#[function_component(FilterBar)]
pub fn filter_bar(props: &FilterBarProps) -> Html {
    let filters = use_filters();
    let apply = use_apply_filters();
    let projects = use_context::<ProjectsContext>();

    // ---------------------------------------------------------------- chips
    let project_name = |id: &str| -> String {
        projects
            .as_ref()
            .and_then(|c| c.projects.iter().find(|p| p.id == id))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| id.to_string())
    };

    let dim_applies = |dim: Dim| {
        if props.restricted {
            dim.applies_to_exceptions()
        } else {
            dim.applies_to_dashboard()
        }
    };

    let chips = filters.query.terms.iter().map(|(dim, value)| {
        let display = display_value(*dim, value, &project_name);
        let inert = !dim_applies(*dim);
        let onremove = {
            let (apply, filters, dim) = (apply.clone(), filters.clone(), *dim);
            Callback::from(move |_: MouseEvent| apply.emit(filters.without(dim)))
        };
        let title = if inert {
            format!("{}: {display} — this page's events don't carry this dimension, so it is not applied here", dim.label())
        } else {
            format!("{}: {display}", dim.label())
        };
        html! {
            <span key={dim.param()} class={classes!("chip", inert.then_some("chip--inert"))} title={title}>
                <span class="chip__dim">{ dim.label() }</span>
                <span class="chip__value">{ display }</span>
                <button class="chip__remove" aria-label={format!("Remove {} filter", dim.label())} onclick={onremove}>
                    { icons::close() }
                </button>
            </span>
        }
    });

    // The advanced (non-chippable) remainder of the query expression renders
    // as one chip; editing happens in the header query bar.
    let advanced_chip = filters.query.advanced.as_ref().map(|advanced| {
        let inert = if props.restricted {
            !filters.advanced_applies_to_exceptions()
        } else {
            !filters.advanced_applies_to_dashboard()
        };
        let onremove = {
            let (apply, filters) = (apply.clone(), filters.clone());
            Callback::from(move |_: MouseEvent| apply.emit(filters.without_advanced()))
        };
        let title = if inert {
            format!("{advanced} — references fields this page's events don't carry, so it is not applied here")
        } else {
            format!("{advanced} — edit in the query bar above")
        };
        html! {
            <span class={classes!("chip", "chip--advanced", inert.then_some("chip--inert"))} title={title}>
                <span class="chip__dim">{ "Query" }</span>
                <span class="chip__value"><code>{ advanced.clone() }</code></span>
                <button class="chip__remove" aria-label="Remove query expression" onclick={onremove}>
                    { icons::close() }
                </button>
            </span>
        }
    });

    let clear_all = (!filters.query.is_empty()).then(|| {
        let (apply, filters) = (apply.clone(), filters.clone());
        let onclick = Callback::from(move |_: MouseEvent| {
            apply.emit(FilterSet {
                range: filters.range,
                query: Default::default(),
            })
        });
        html! { <button class="filter-bar__clear" onclick={onclick}>{ "Clear" }</button> }
    });

    // ---------------------------------------------------------------- range
    let mut range_items: Vec<DropdownItem> = RangePreset::ALL
        .iter()
        .map(|p| DropdownItem::new(p.token(), p.label()))
        .collect();
    let range_value = match filters.range {
        TimeRange::Preset(preset) => preset.token().to_string(),
        TimeRange::Custom { from, to } => {
            range_items.push(DropdownItem::new(
                "custom",
                format!("{} – {}", short_date(from), short_date(to)),
            ));
            "custom".to_string()
        }
    };
    let on_range = {
        let (apply, filters) = (apply.clone(), filters.clone());
        Callback::from(move |token: String| {
            if let Some(preset) = RangePreset::from_token(&token) {
                apply.emit(filters.with_range(TimeRange::Preset(preset)));
            }
        })
    };

    html! {
        <div class="filter-bar">
            <div class="filter-bar__chips">
                { for chips }
                { advanced_chip }
                <AddFilter
                    filters={filters.clone()}
                    suggestions={props.suggestions.clone()}
                    restricted={props.restricted}
                />
                { clear_all }
            </div>
            <div class="filter-bar__range">
                <Dropdown items={range_items} value={range_value} on_select={on_range} />
            </div>
        </div>
    }
}

/// The human form of a filter value for its chip (and panel labels).
pub fn display_value(dim: Dim, value: &str, project_name: &impl Fn(&str) -> String) -> String {
    if value.is_empty() {
        return dim.absent_label().to_string();
    }
    match dim {
        Dim::Project => project_name(value),
        Dim::Country => match country_flag(value) {
            Some(flag) => format!("{flag} {}", country_name(value)),
            None => country_name(value),
        },
        Dim::Language => language_name(value),
        Dim::Source => analytics_api::source_label(value).to_string(),
        _ => value.to_string(),
    }
}

#[derive(Properties, PartialEq)]
struct AddFilterProps {
    filters: FilterSet,
    suggestions: Rc<Vec<(Dim, Vec<SuggestOption>)>>,
    restricted: bool,
}

/// The "+ Filter" popover: pick a dimension, then a value (free text or one of
/// the suggested values — top breakdown rows, the project list, …).
#[function_component(AddFilter)]
fn add_filter(props: &AddFilterProps) -> Html {
    let open = use_state(|| false);
    let picked = use_state(|| None::<Dim>);
    let text = use_state(String::new);
    let apply = use_apply_filters();
    let input_ref = use_node_ref();

    let close = {
        let (open, picked, text) = (open.clone(), picked.clone(), text.clone());
        Callback::from(move |_: MouseEvent| {
            open.set(false);
            picked.set(None);
            text.set(String::new());
        })
    };
    let toggle = {
        let (open, picked, text) = (open.clone(), picked.clone(), text.clone());
        Callback::from(move |_: MouseEvent| {
            if !*open {
                picked.set(None);
                text.set(String::new());
            }
            open.set(!*open);
        })
    };

    // Focus the value input as soon as a dimension is picked.
    {
        let input_ref = input_ref.clone();
        use_effect_with(*picked, move |picked| {
            if picked.is_some()
                && let Some(input) = input_ref.cast::<HtmlInputElement>()
            {
                let _ = input.focus();
            }
        });
    }

    let submit = {
        let (apply, filters) = (apply.clone(), props.filters.clone());
        let (open, picked, text) = (open.clone(), picked.clone(), text.clone());
        move |dim: Dim, value: String| {
            apply.emit(filters.with(dim, value));
            open.set(false);
            picked.set(None);
            text.set(String::new());
        }
    };

    let dims = Dim::ALL.into_iter().filter(|d| {
        if props.restricted {
            d.applies_to_exceptions()
        } else {
            d.applies_to_dashboard()
        }
    });

    let body = match *picked {
        None => {
            let items = dims.map(|dim| {
                let onclick = {
                    let picked = picked.clone();
                    Callback::from(move |_: MouseEvent| picked.set(Some(dim)))
                };
                html! {
                    <li key={dim.param()}>
                        <button class="add-filter__dim" onclick={onclick}>
                            { dim.label() }
                            <span class="add-filter__chev">{ icons::chevron_right() }</span>
                        </button>
                    </li>
                }
            });
            html! { <ul class="add-filter__list">{ for items }</ul> }
        }
        Some(dim) => {
            let needle = text.to_lowercase();
            let options: Vec<SuggestOption> = props
                .suggestions
                .iter()
                .find(|(d, _)| *d == dim)
                .map(|(_, options)| options.clone())
                .unwrap_or_default()
                .into_iter()
                .filter(|option| {
                    needle.is_empty()
                        || option.label.to_lowercase().contains(&needle)
                        || option.value.to_lowercase().contains(&needle)
                })
                .take(8)
                .collect();

            let oninput = {
                let text = text.clone();
                Callback::from(move |e: InputEvent| {
                    text.set(e.target_unchecked_into::<HtmlInputElement>().value());
                })
            };
            let onkeydown = {
                let submit = submit.clone();
                let (text, options) = (text.clone(), options.clone());
                Callback::from(move |e: KeyboardEvent| {
                    if e.key() == "Enter" {
                        e.prevent_default();
                        let typed = text.trim().to_string();
                        if !typed.is_empty() {
                            submit(dim, typed);
                        } else if let Some(first) = options.first() {
                            submit(dim, first.value.clone());
                        }
                    }
                })
            };
            let back = {
                let picked = picked.clone();
                Callback::from(move |_: MouseEvent| picked.set(None))
            };

            let option_items = options.iter().enumerate().map(|(i, option)| {
                let onclick = {
                    let submit = submit.clone();
                    let value = option.value.clone();
                    Callback::from(move |_: MouseEvent| submit(dim, value.clone()))
                };
                html! {
                    <li key={i.to_string()}>
                        <button class="add-filter__option" onclick={onclick}>
                            <span class={classes!(option.value.is_empty().then_some("brow__text--absent"))}>
                                { option.label.clone() }
                            </span>
                        </button>
                    </li>
                }
            });

            html! {
                <div class="add-filter__values">
                    <div class="add-filter__picked">
                        <button class="add-filter__back" onclick={back} aria-label="Back">
                            { icons::chevron_left() }
                        </button>
                        <span>{ dim.label() }</span>
                    </div>
                    <input
                        ref={input_ref.clone()}
                        class="input add-filter__input"
                        placeholder={format!("Filter by {}…", dim.label().to_lowercase())}
                        value={(*text).clone()}
                        oninput={oninput}
                        onkeydown={onkeydown}
                    />
                    if options.is_empty() && text.is_empty() {
                        <p class="add-filter__hint">{ "Type a value and press Enter." }</p>
                    } else {
                        <ul class="add-filter__list">{ for option_items }</ul>
                    }
                </div>
            }
        }
    };

    html! {
        <div class="add-filter">
            <button class="chip chip--add" onclick={toggle} aria-expanded={open.to_string()}>
                { icons::plus() }
                { "Filter" }
            </button>
            if *open {
                <div class="add-filter__backdrop" onclick={close} />
                <div class="add-filter__pop">{ body }</div>
            }
        </div>
    }
}
