//! The lookback selector shown in page headers. It resolves a preset into the
//! `from`/`to`/`interval` query the stats endpoints expect.

use yew::prelude::*;

use crate::components::{Dropdown, DropdownItem};

/// A lookback window: a number of days back, paired with a sensible bucket size.
#[derive(Clone, Copy, PartialEq)]
pub struct Range {
    pub days: i64,
    pub interval: &'static str,
}

/// `(days, interval, label)` presets, in display order. Buckets coarsen with the
/// window: 24h hourly, 7d in 6-hour windows, 30d/90d daily, a year in weeks.
const PRESETS: &[(i64, &str, &str)] = &[
    (1, "hour", "Last 24 hours"),
    (7, "6h", "Last 7 days"),
    (30, "day", "Last 30 days"),
    (90, "day", "Last 90 days"),
    (365, "week", "Last 12 months"),
];

impl Range {
    /// The default lookback (7 days, daily buckets).
    pub fn week() -> Range {
        Range { days: 7, interval: "day" }
    }

    /// The `from`/`to`/`interval` query string for this window, anchored to now.
    pub fn query(&self) -> String {
        let now = js_sys::Date::now() as i64;
        let from = now - self.days * 86_400_000;
        format!("from={from}&to={now}&interval={}", self.interval)
    }
}

#[derive(Properties, PartialEq)]
pub struct RangePickerProps {
    pub value: Range,
    pub on_change: Callback<Range>,
}

#[function_component(RangePicker)]
pub fn range_picker(props: &RangePickerProps) -> Html {
    let items: Vec<DropdownItem> =
        PRESETS.iter().map(|(d, _, label)| DropdownItem::new(d.to_string(), *label)).collect();

    let on_select = {
        let on_change = props.on_change.clone();
        Callback::from(move |value: String| {
            if let Some((days, interval, _)) = PRESETS.iter().find(|(d, _, _)| d.to_string() == value) {
                on_change.emit(Range { days: *days, interval });
            }
        })
    };

    html! { <Dropdown items={items} value={props.value.days.to_string()} {on_select} /> }
}
