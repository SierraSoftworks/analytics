//! One distribution card: proportional bars over a dimension's values, shared
//! by the exception and event detail pages.

use analytics_api::CountRow;
use yew::prelude::*;

use crate::format::compact;

/// Render a distribution card. Cards whose only row is the absent sentinel
/// carry no signal and are dropped.
pub fn distribution(title: &str, rows: &[CountRow], total: i64) -> Html {
    let informative = rows.iter().any(|r| !r.key.is_empty());
    if rows.is_empty() || !informative {
        return html! {};
    }
    let max = rows.iter().map(|r| r.count).max().unwrap_or(1).max(1);
    let total = total.max(1);
    let bars = rows.iter().map(|row| {
        let share = row.count as f64 / total as f64 * 100.0;
        let width = row.count as f64 / max as f64 * 100.0;
        let label = if row.key.is_empty() {
            "Unknown".to_string()
        } else {
            row.key.clone()
        };
        html! {
            <li class="dist__row" key={row.key.clone()}>
                <span class="dist__bar" style={format!("width: {width:.1}%")} />
                <span class={classes!("dist__label", row.key.is_empty().then_some("brow__text--absent"))}
                    title={label.clone()}>
                    { label }
                </span>
                <span class="dist__share">{ format!("{share:.0}%") }</span>
                <span class="dist__count">{ compact(row.count) }</span>
            </li>
        }
    });
    html! {
        <section class="dist">
            <h3 class="dist__title">{ title.to_string() }</h3>
            <ul class="dist__rows">{ for bars }</ul>
        </section>
    }
}
