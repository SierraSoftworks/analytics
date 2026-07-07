//! Tabbed breakdown panels: every row is a click-to-filter control with a
//! proportional bar behind it, a share percentage, and an explicit affordance
//! (hover highlight + funnel icon), following the Plausible/Sentry convention.

use yew::prelude::*;

use crate::components::charts::Metric;
use crate::components::icons;
use crate::filters::Dim;
use crate::format::compact;

/// One prepared row of a breakdown panel. Pages convert `BreakdownRow` API rows
/// into these, resolving display labels (project names, country flags, sentinel
/// values) while keeping the raw filter value for click-through.
#[derive(Clone, PartialEq)]
pub struct PanelRow {
    /// The raw value the row filters by (empty = the "absent" sentinel).
    pub value: String,
    pub label: String,
    /// An optional leading glyph (e.g. a country flag).
    pub icon: Option<String>,
    pub visitors: i64,
    pub pageviews: i64,
    pub events: i64,
    /// Hovering the label shows this (defaults to the label).
    pub title: Option<String>,
    /// An additional filter term applied (and toggled) together with the
    /// row's main value — e.g. a version row also pins its application.
    pub extra: Option<(Dim, String)>,
}

impl PanelRow {
    fn value_for(&self, metric: Metric) -> i64 {
        match metric {
            Metric::Visitors => self.visitors,
            Metric::Pageviews => self.pageviews,
            Metric::Events => self.events,
        }
    }
}

/// One tab of a panel: a labelled set of rows filtering one dimension.
#[derive(Clone, PartialEq)]
pub struct PanelTab {
    pub label: &'static str,
    pub dim: Dim,
    pub rows: Vec<PanelRow>,
    /// Optional per-row action (e.g. "manage project") rendered at the row end.
    pub action: Option<Callback<String>>,
    /// The icon title for the per-row action.
    pub action_title: &'static str,
}

impl PanelTab {
    pub fn new(label: &'static str, dim: Dim, rows: Vec<PanelRow>) -> Self {
        Self {
            label,
            dim,
            rows,
            action: None,
            action_title: "",
        }
    }

    pub fn with_action(mut self, title: &'static str, action: Callback<String>) -> Self {
        self.action = Some(action);
        self.action_title = title;
        self
    }
}

#[derive(Properties, PartialEq)]
pub struct BreakdownPanelProps {
    pub tabs: Vec<PanelTab>,
    pub metric: Metric,
    /// Invoked with the row's filter terms — `(dim, raw value)` pairs, the
    /// tab's dimension first — when a row is clicked.
    pub on_filter: Callback<Vec<(Dim, String)>>,
    /// The active filter value per dimension (highlights the selected row).
    #[prop_or_default]
    pub active: Vec<(Dim, String)>,
}

/// Rows shown per panel before the "Show all" affordance takes over — enough
/// to read the shape of a distribution without letting a 25-row breakdown
/// stretch the page.
const VISIBLE_ROWS: usize = 8;

#[function_component(BreakdownPanel)]
pub fn breakdown_panel(props: &BreakdownPanelProps) -> Html {
    let tab_index = use_state(|| 0usize);
    let expanded = use_state(|| false);
    let index = (*tab_index).min(props.tabs.len().saturating_sub(1));
    let Some(tab) = props.tabs.get(index) else {
        return html! {};
    };

    let tabs = props.tabs.iter().enumerate().map(|(i, t)| {
        let active = i == index;
        let onclick = {
            let (tab_index, expanded) = (tab_index.clone(), expanded.clone());
            Callback::from(move |_: MouseEvent| {
                tab_index.set(i);
                expanded.set(false);
            })
        };
        html! {
            <button key={i.to_string()}
                class={classes!("panel-tab", active.then_some("panel-tab--active"))}
                onclick={onclick}>
                { t.label }
            </button>
        }
    });

    // Dimension tabs carry no events at all; when the Events metric is active
    // there, fall back to page views for the *whole tab* (mixing real event
    // counts with pageview fallbacks per-row would rank apples against
    // oranges). Tabs with any real events display true event counts.
    let tab_has_events = tab.rows.iter().any(|r| r.events > 0);
    let metric = if props.metric == Metric::Events && !tab_has_events {
        Metric::Pageviews
    } else {
        props.metric
    };
    // The API orders rows by pageviews; re-rank by whichever metric is displayed.
    let mut ranked: Vec<&PanelRow> = tab.rows.iter().collect();
    ranked.sort_by_key(|r| std::cmp::Reverse(r.value_for(metric)));
    let total: i64 = ranked.iter().map(|r| r.value_for(metric)).sum();
    let max: i64 = ranked
        .iter()
        .map(|r| r.value_for(metric))
        .max()
        .unwrap_or(1)
        .max(1);
    // Case-insensitive, like the server's string comparisons, so a hand-typed
    // `project == "apps"` still highlights the "Apps" row.
    let selected = props
        .active
        .iter()
        .find(|(d, _)| *d == tab.dim)
        .map(|(_, v)| v.to_lowercase());

    let hidden = ranked.len().saturating_sub(VISIBLE_ROWS);
    let shown = if *expanded || hidden == 0 {
        ranked.len()
    } else {
        VISIBLE_ROWS
    };

    let rows = ranked.iter().take(shown).enumerate().map(|(i, row)| {
        let value = row.value_for(metric);
        let share = if total > 0 { value as f64 / total as f64 * 100.0 } else { 0.0 };
        let bar = if max > 0 { value as f64 / max as f64 * 100.0 } else { 0.0 };
        let is_selected = selected.as_deref() == Some(row.value.to_lowercase().as_str())
            && row.extra.as_ref().is_none_or(|(dim, value)| {
                props
                    .active
                    .iter()
                    .any(|(d, v)| d == dim && v.to_lowercase() == value.to_lowercase())
            });

        let onclick = {
            let on_filter = props.on_filter.clone();
            let mut terms = vec![(tab.dim, row.value.clone())];
            terms.extend(row.extra.clone());
            Callback::from(move |_: MouseEvent| on_filter.emit(terms.clone()))
        };
        // The action is a sibling of the row button (never a child — nested
        // interactive elements are invalid HTML and confuse assistive tech),
        // absolutely positioned over the row's end on hover.
        let action = tab.action.as_ref().map(|action| {
            let (action, value) = (action.clone(), row.value.clone());
            let onclick = Callback::from(move |_: MouseEvent| action.emit(value.clone()));
            html! {
                <button class="brow__action" title={tab.action_title} onclick={onclick}>
                    { icons::gear() }
                </button>
            }
        });

        html! {
            <li class="brow-item" key={i.to_string()}>
                <button
                    class={classes!("brow", is_selected.then_some("brow--selected"))}
                    onclick={onclick}
                    title={format!("Filter: {} is {}", tab.dim.label(), row.title.clone().unwrap_or_else(|| row.label.clone()))}
                >
                    <span class="brow__bar" style={format!("width: {bar:.1}%")} />
                    <span class="brow__label">
                        if let Some(icon) = &row.icon {
                            <span class="brow__icon">{ icon.clone() }</span>
                        }
                        <span class={classes!("brow__text", row.value.is_empty().then_some("brow__text--absent"))}>
                            { row.label.clone() }
                        </span>
                        <span class="brow__filter-hint">{ icons::filter() }</span>
                    </span>
                    <span class="brow__share">{ format!("{share:.0}%") }</span>
                    <span class="brow__count">{ compact(value) }</span>
                </button>
                { action }
            </li>
        }
    });

    html! {
        <section class="panel-card">
            <header class="panel-card__head">
                <div class="panel-card__tabs">{ for tabs }</div>
                <span class="panel-card__metric">{ metric_note(metric, tab) }</span>
            </header>
            if tab.rows.is_empty() {
                <div class="panel-card__empty">{ "No data in this period." }</div>
            } else {
                <ul class="panel-card__rows">{ for rows }</ul>
                if hidden > 0 {
                    <button class="panel-card__more" onclick={{
                        let expanded = expanded.clone();
                        Callback::from(move |_: MouseEvent| expanded.set(!*expanded))
                    }}>
                        { if *expanded {
                            "Show fewer".to_string()
                        } else {
                            format!("Show all {}", ranked.len())
                        } }
                    </button>
                }
            }
        </section>
    }
}

fn metric_note(metric: Metric, _tab: &PanelTab) -> &'static str {
    match metric {
        Metric::Visitors => "visitors",
        Metric::Pageviews => "views",
        Metric::Events => "events",
    }
}
