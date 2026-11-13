//! The persistent filter bar: active filter chips (removable, one per
//! dimension) and the add-filter popover, plus the time-range picker pages put
//! in their header. All state lives in the URL via [`crate::filters`]; these
//! components only render it and push changes.

use yew::prelude::*;
use yewi::prelude::{
    Button, ButtonVariant, IconButton, Menu, MenuItem, Placement, Popover, Select, SelectOption,
    Size, Tag, Text, TextColor, TextInput, TextSize, TextTag, attrs, icons,
};

use crate::components::ProjectsContext;
use crate::components::shell::{Suggestions, SuggestionsContext};
use crate::filters::{Dim, FilterSet, RangePreset, TimeRange, use_apply_filters, use_filters};
use crate::format::{country_flag, country_name, language_name, short_date};

/// One suggested value for the add-filter popover and the query bar.
#[derive(Clone, PartialEq)]
pub struct SuggestOption {
    pub value: String,
    pub label: String,
}

#[derive(Properties, PartialEq)]
pub struct FilterBarProps {
    /// Per-dimension value suggestions (typically the current breakdown rows).
    #[prop_or_default]
    pub suggestions: Suggestions,
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

    // The query bar in the app bar completes the same values.
    {
        let publish = use_context::<SuggestionsContext>();
        use_effect_with(props.suggestions.clone(), move |suggestions| {
            if let Some(ctx) = &publish {
                ctx.set.emit(suggestions.clone());
            }
            move || {
                if let Some(ctx) = publish {
                    ctx.set.emit(Suggestions::default());
                }
            }
        });
    }

    // Project filter values are names (matched case-insensitively, like the
    // server); ids are still resolved so pre-rename links display properly.
    let project_name = |value: &str| -> String {
        let needle = value.to_lowercase();
        projects
            .as_ref()
            .and_then(|c| {
                c.projects
                    .iter()
                    .find(|p| p.name.to_lowercase() == needle || p.id == value)
            })
            .map(|p| p.name.clone())
            .unwrap_or_else(|| value.to_string())
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
        let on_remove = {
            let (apply, filters, dim) = (apply.clone(), filters.clone(), *dim);
            Callback::from(move |_: MouseEvent| apply.emit(filters.without(dim)))
        };
        let title = if inert {
            format!("{}: {display}. This page's events don't carry this dimension, so it is not applied here.", dim.label())
        } else {
            format!("{}: {display}", dim.label())
        };
        html! {
            <Tag key={dim.param()} name={dim.label()} {on_remove}
                remove_label={format!("Remove {} filter", dim.label())}
                class={classes!(inert.then_some("chip--inert"))} attrs={attrs! { "title" => title }}>
                { display }
            </Tag>
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
        let on_remove = {
            let (apply, filters) = (apply.clone(), filters.clone());
            Callback::from(move |_: MouseEvent| apply.emit(filters.without_advanced()))
        };
        let title = if inert {
            format!("{advanced}. References fields this page's events don't carry, so it is not applied here.")
        } else {
            format!("{advanced}. Edit it in the query bar above.")
        };
        html! {
            <Tag name="Query" mono=true {on_remove} remove_label="Remove query expression"
                class={classes!("chip--advanced", inert.then_some("chip--inert"))}
                attrs={attrs! { "title" => title }}>
                { advanced.clone() }
            </Tag>
        }
    });

    let clear_all = (!filters.query.is_empty()).then(|| {
        let (apply, filters) = (apply.clone(), filters.clone());
        let on_click = Callback::from(move |_: MouseEvent| {
            apply.emit(FilterSet {
                range: filters.range,
                query: Default::default(),
            })
        });
        html! { <Button variant={ButtonVariant::Subtle} size={Size::Small} {on_click}>{ "Clear" }</Button> }
    });

    html! {
        <div class="filter-bar">
            { for chips }
            { advanced_chip }
            <AddFilter
                filters={filters.clone()}
                suggestions={props.suggestions.clone()}
                restricted={props.restricted}
            />
            { clear_all }
        </div>
    }
}

/// The time-range picker: the presets, plus the custom range a chart zoom
/// produced while one is active.
#[function_component(RangeSelect)]
pub fn range_select() -> Html {
    let filters = use_filters();
    let apply = use_apply_filters();

    let mut options: Vec<SelectOption> = RangePreset::ALL
        .iter()
        .map(|p| SelectOption::new(p.token(), p.label()))
        .collect();
    let value = match filters.range {
        TimeRange::Preset(preset) => preset.token().to_string(),
        TimeRange::Custom { from, to } => {
            options.push(SelectOption::new(
                "custom",
                format!("{} – {}", short_date(from), short_date(to)),
            ));
            "custom".to_string()
        }
    };
    let on_change = Callback::from(move |token: AttrValue| {
        if let Some(preset) = RangePreset::from_token(&token) {
            apply.emit(filters.with_range(TimeRange::Preset(preset)));
        }
    });

    html! { <Select {options} value={AttrValue::from(value)} {on_change} label="Time range" fit=true /> }
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
    suggestions: Suggestions,
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

    let on_open_change = {
        let (open, picked, text) = (open.clone(), picked.clone(), text.clone());
        Callback::from(move |next: bool| {
            picked.set(None);
            text.set(String::new());
            open.set(next);
        })
    };

    // Focus the value input as soon as a dimension is picked.
    {
        let input_ref = input_ref.clone();
        use_effect_with(*picked, move |picked| {
            if picked.is_some()
                && let Some(input) = input_ref.cast::<web_sys::HtmlElement>()
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

    let content = match *picked {
        None => {
            let items = dims.map(|dim| {
                let on_select = {
                    let picked = picked.clone();
                    Callback::from(move |_: MouseEvent| picked.set(Some(dim)))
                };
                html! { <MenuItem key={dim.param()} {on_select}>{ dim.label() }</MenuItem> }
            });
            html! { <Menu label="Filter by">{ for items }</Menu> }
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

            let on_input = {
                let text = text.clone();
                Callback::from(move |value: String| text.set(value))
            };
            let on_keydown = {
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
                let on_select = {
                    let submit = submit.clone();
                    let value = option.value.clone();
                    Callback::from(move |_: MouseEvent| submit(dim, value.clone()))
                };
                html! { <MenuItem key={i.to_string()} {on_select}>{ option.label.clone() }</MenuItem> }
            });

            html! {
                <div class="add-filter">
                    <div class="add-filter__picked">
                        <IconButton icon={icons::CHEVRON_LEFT} label="Back" size={Size::Small}
                            variant={ButtonVariant::Subtle} on_click={back} />
                        <Text weight={yewi::TextWeight::Semibold}>{ dim.label() }</Text>
                    </div>
                    <TextInput
                        input_ref={input_ref.clone()}
                        size={Size::Small}
                        placeholder={format!("Filter by {}…", dim.label().to_lowercase())}
                        value={(*text).clone()}
                        {on_input}
                        {on_keydown}
                        attrs={attrs! { "aria-label" => format!("{} value", dim.label()) }}
                    />
                    if options.is_empty() {
                        <Text tag={TextTag::P} size={TextSize::Sm} color={TextColor::Subtle}>
                            { "Type a value and press Enter." }
                        </Text>
                    } else {
                        <Menu label={format!("{} values", dim.label())}>{ for option_items }</Menu>
                    }
                </div>
            }
        }
    };

    html! {
        <Popover open={*open} {on_open_change} {content} placement={Placement::BottomStart}
            arrow=false flush=true label="Add a filter">
            <Button size={Size::Small} icon={icons::PLUS}>{ "Filter" }</Button>
        </Popover>
    }
}
