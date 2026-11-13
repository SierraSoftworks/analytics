//! Steps through a detail page's distinct examples one at a time.

use yew::prelude::*;
use yewi::prelude::{Heading, HeadingLevel, Pagination, Size, TextSize};

#[derive(Properties, PartialEq)]
pub struct ScrubberProps {
    /// How many examples there are.
    pub count: usize,
    /// The example shown, counting from zero.
    pub index: usize,
    pub on_change: Callback<usize>,
    /// The example itself.
    #[prop_or_default]
    pub children: Html,
}

/// The "Distinct examples" section: its heading, the previous/next control,
/// and the example in view.
#[function_component(Scrubber)]
pub fn scrubber(props: &ScrubberProps) -> Html {
    let on_change = props
        .on_change
        .reform(|page: u32| page.saturating_sub(1) as usize);
    html! {
        <section class="section">
            <header class="section__head">
                <Heading level={HeadingLevel::H2} size={TextSize::Md}>{ "Distinct examples" }</Heading>
                if props.count > 1 {
                    <Pagination page={props.index as u32 + 1} pages={props.count as u32} compact=true
                        size={Size::Small} {on_change} label="Examples" />
                }
            </header>
            <div class="example">{ props.children.clone() }</div>
        </section>
    }
}
