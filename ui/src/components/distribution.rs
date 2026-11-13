//! One distribution list: proportional bars over a dimension's values, shared
//! by the exception and event detail pages.

use std::rc::Rc;

use analytics_api::CountRow;
use yew::prelude::*;
use yewi::prelude::{BarItem, BarList, BarListOverflow, Heading, HeadingLevel, TextSize, Tone};

/// Render a distribution. Lists whose only row is the absent sentinel carry no
/// signal and are dropped. `tone` colours the bars when the counts are trouble
/// (exceptions) rather than traffic.
pub fn distribution(title: &str, rows: &[CountRow], total: i64, tone: Tone) -> Html {
    let informative = rows.iter().any(|r| !r.key.is_empty());
    if rows.is_empty() || !informative {
        return html! {};
    }
    let items: Rc<[BarItem]> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            if row.key.is_empty() {
                BarItem::new(i.to_string(), "Unknown", row.count as f64).muted()
            } else {
                BarItem::new(i.to_string(), row.key.clone(), row.count as f64)
            }
        })
        .collect();
    html! {
        <section class="dist" key={title.to_string()}>
            <Heading level={HeadingLevel::H3} size={TextSize::Sm}>{ title.to_string() }</Heading>
            <BarList {items} label={title.to_string()} {tone} show_share=true
                total={total.max(1) as f64} overflow={BarListOverflow::Expand(6)} />
        </section>
    }
}
