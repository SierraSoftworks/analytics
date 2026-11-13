//! Tabbed breakdown panels: every row is a click-to-filter control with a
//! proportional bar behind it and a share percentage, following the
//! Plausible/Sentry convention. The tabs and the ranked rows are yewi's `Tabs`
//! and `BarList`; this module prepares the rows and maps a click back to the
//! filter terms it stands for.

use std::rc::Rc;

use yew::prelude::*;
use yewi::prelude::{
    BarItem, BarList, BarListAction, BarListOverflow, IconData, Size, Tab, Tabs, TabsVariant, Text,
    TextColor, TextSize, TextTag, icons,
};

use crate::components::charts::Metric;
use crate::filters::Dim;

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

/// The glyph rendered on a tab's per-row action.
#[derive(Clone, Copy, PartialEq)]
pub enum ActionIcon {
    /// Configure the row's entity (e.g. "manage project").
    Gear,
    /// Open the row's detail page.
    Open,
}

impl ActionIcon {
    fn icon(self) -> IconData {
        match self {
            ActionIcon::Gear => icons::SETTINGS,
            ActionIcon::Open => icons::CHEVRON_RIGHT,
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
    /// The glyph for the per-row action (a gear unless overridden).
    pub action_icon: ActionIcon,
}

impl PanelTab {
    pub fn new(label: &'static str, dim: Dim, rows: Vec<PanelRow>) -> Self {
        Self {
            label,
            dim,
            rows,
            action: None,
            action_title: "",
            action_icon: ActionIcon::Gear,
        }
    }

    pub fn with_action(mut self, title: &'static str, action: Callback<String>) -> Self {
        self.action = Some(action);
        self.action_title = title;
        self
    }

    pub fn with_action_icon(mut self, icon: ActionIcon) -> Self {
        self.action_icon = icon;
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
    /// When set, the panel adopts this tab index whenever the value *changes*
    /// (e.g. the pages panel follows the dashboard metric to its Events tab).
    /// Manual tab clicks still work in between changes.
    #[prop_or_default]
    pub select_tab: Option<usize>,
}

/// Rows shown per panel before the "Show all" affordance takes over — enough
/// to read the shape of a distribution without letting a 25-row breakdown
/// stretch the page.
const VISIBLE_ROWS: usize = 8;

#[function_component(BreakdownPanel)]
pub fn breakdown_panel(props: &BreakdownPanelProps) -> Html {
    let tab_index = use_state(|| props.select_tab.unwrap_or(0));

    // Follow externally-driven tab selection (fires only when the value
    // changes, so it never overrides a manual click in between).
    {
        let tab_index = tab_index.clone();
        use_effect_with(props.select_tab, move |select| {
            if let Some(i) = *select
                && *tab_index != i
            {
                tab_index.set(i);
            }
            || ()
        });
    }

    let index = (*tab_index).min(props.tabs.len().saturating_sub(1));
    let on_change = {
        let tab_index = tab_index.clone();
        Callback::from(move |value: AttrValue| tab_index.set(value.parse().unwrap_or(0)))
    };

    let tabs = props.tabs.iter().enumerate().map(|(i, tab)| {
        html_nested! {
            <Tab key={i.to_string()} value={i.to_string()} label={tab.label}>
                // Only the open tab's rows are worth preparing.
                if i == index {
                    <TabRows tab={tab.clone()} metric={props.metric}
                        on_filter={props.on_filter.clone()} active={props.active.clone()} />
                }
            </Tab>
        }
    });

    html! {
        <section class="breakdown">
            <Tabs selected={index.to_string()} {on_change} variant={TabsVariant::Pill}
                size={Size::Small} label="Breakdown">
                { for tabs }
            </Tabs>
        </section>
    }
}

#[derive(Properties, PartialEq)]
struct TabRowsProps {
    tab: PanelTab,
    metric: Metric,
    on_filter: Callback<Vec<(Dim, String)>>,
    active: Vec<(Dim, String)>,
}

/// One tab's ranked rows. A row is addressed by its position in the tab,
/// because a filter value alone need not be unique (two applications can share
/// a version number).
#[function_component(TabRows)]
fn tab_rows(props: &TabRowsProps) -> Html {
    let tab = &props.tab;

    // Most dimension tabs carry no events at all; when the Events metric is
    // active there, fall back to page views for the *whole tab* (mixing real
    // event counts with pageview fallbacks per-row would rank apples against
    // oranges). The Events tab is the mirror image — its rows carry nothing
    // *but* events — so a view metric falls back to event counts there.
    let tab_has_events = tab.rows.iter().any(|r| r.events > 0);
    let tab_has_views = tab.rows.iter().any(|r| r.pageviews > 0 || r.visitors > 0);
    let metric = match props.metric {
        Metric::Events if !tab_has_events => Metric::Pageviews,
        m if m != Metric::Events && !tab_has_views && tab_has_events => Metric::Events,
        m => m,
    };

    // The API orders rows by pageviews; BarList re-ranks by the amount shown.
    let items: Rc<[BarItem]> = tab
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let item = BarItem::new(
                i.to_string(),
                row.label.clone(),
                row.value_for(metric) as f64,
            );
            let item = match &row.icon {
                Some(glyph) => item.glyph(glyph.clone()),
                None => item,
            };
            // The "absent" sentinel reads as a gap, not a value.
            if row.value.is_empty() {
                item.muted()
            } else {
                item
            }
        })
        .collect();

    // Case-insensitive, like the server's string comparisons, so a hand-typed
    // `project == "apps"` still highlights the "Apps" row.
    let is_active = |dim: Dim, value: &str| {
        props
            .active
            .iter()
            .any(|(d, v)| *d == dim && v.to_lowercase() == value.to_lowercase())
    };
    let selected = tab
        .rows
        .iter()
        .position(|row| {
            is_active(tab.dim, &row.value)
                && row
                    .extra
                    .as_ref()
                    .is_none_or(|(dim, value)| is_active(*dim, value))
        })
        .map(|i| AttrValue::from(i.to_string()));

    let rows = Rc::new(tab.rows.clone());
    let on_select = {
        let (rows, on_filter, dim) = (rows.clone(), props.on_filter.clone(), tab.dim);
        Callback::from(move |key: AttrValue| {
            if let Some(row) = key.parse::<usize>().ok().and_then(|i| rows.get(i)) {
                let mut terms = vec![(dim, row.value.clone())];
                terms.extend(row.extra.clone());
                on_filter.emit(terms);
            }
        })
    };
    // Sentinel rows aggregate absent values, which no detail page can address.
    let action = tab.action.clone().map(|action| {
        let rows = rows.clone();
        let on_click = Callback::from(move |key: AttrValue| {
            if let Some(row) = key.parse::<usize>().ok().and_then(|i| rows.get(i))
                && !row.value.is_empty()
            {
                action.emit(row.value.clone());
            }
        });
        BarListAction::new(tab.action_icon.icon(), tab.action_title, on_click)
    });

    let empty = html! {
        <Text tag={TextTag::P} size={TextSize::Sm} color={TextColor::Subtle} class="breakdown__empty">
            { "No data in this period." }
        </Text>
    };

    html! {
        <BarList
            {items}
            label={tab.label}
            value_heading={metric_note(metric)}
            overflow={BarListOverflow::Expand(VISIBLE_ROWS)}
            show_share=true
            {selected}
            {on_select}
            {action}
            {empty}
        />
    }
}

fn metric_note(metric: Metric) -> &'static str {
    match metric {
        Metric::Visitors => "Visitors",
        Metric::Pageviews => "Views",
        Metric::Events => "Events",
    }
}
