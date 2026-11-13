//! Small pieces every routed page shares.

use yew::prelude::*;
use yewi::prelude::{
    Breadcrumbs, Crumb, Heading, HeadingLevel, Loading, LoadingLayout, NavTarget, Text, TextColor,
    TextSize, TextTag,
};

use crate::app::Route;
use crate::filters::FilterSet;

/// The trail of a detail page: its parent list, then the page itself. The
/// parent link carries the filter state, so stepping back up the trail reopens
/// the list the way the operator left it.
pub fn crumbs(parent: &'static str, route: Route, filters: &FilterSet, current: &str) -> Html {
    html! {
        <Breadcrumbs>
            <Crumb href={NavTarget::route(route).query(filters.to_pairs())}>{ parent }</Crumb>
            <Crumb current=true>{ current.to_string() }</Crumb>
        </Breadcrumbs>
    }
}

/// The placeholder shown while a page's data is on its way.
pub fn loading() -> Html {
    html! { <Loading label="Loading…" layout={LoadingLayout::Block} delay=true /> }
}

#[derive(Properties, PartialEq)]
pub struct SectionProps {
    pub title: AttrValue,
    /// A line under the title saying what the section is for.
    #[prop_or_default]
    pub description: Option<AttrValue>,
    /// Controls at the end of the title row.
    #[prop_or_default]
    pub actions: Html,
    #[prop_or_default]
    pub children: Html,
}

/// A titled stretch of a page. It sits flat on the page: sections are a
/// page's own content, so they are told apart by their headings and the space
/// between them rather than by a surface each.
#[function_component(Section)]
pub fn section(props: &SectionProps) -> Html {
    html! {
        <section class="section">
            <header class="section__head">
                <Heading level={HeadingLevel::H2} size={TextSize::Md}>{ props.title.clone() }</Heading>
                <div class="section__actions">{ props.actions.clone() }</div>
            </header>
            if let Some(description) = &props.description {
                <Text tag={TextTag::P} size={TextSize::Sm} color={TextColor::Muted} class="section__description">
                    { description.clone() }
                </Text>
            }
            { props.children.clone() }
        </section>
    }
}
