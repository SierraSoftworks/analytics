//! The page header shown at the top of each routed page: an optional breadcrumb
//! trail, the page title and subtitle, and a slot for page-level actions.

use yew::prelude::*;
use yew_router::prelude::*;

use crate::app::Route;

/// One breadcrumb entry. A `to` of `None` renders the (non-link) current page.
#[derive(Clone, PartialEq)]
pub struct Crumb {
    pub label: AttrValue,
    pub to: Option<Route>,
    /// Query pairs carried on the link (e.g. the active filter state, so
    /// stepping back up the trail doesn't silently drop filters).
    pub query: Vec<(String, String)>,
}

impl Crumb {
    /// A link crumb carrying query pairs (usually the active filter state, so
    /// stepping back up the trail doesn't silently drop filters).
    pub fn link_with_query(
        label: impl Into<AttrValue>,
        to: Route,
        query: Vec<(String, String)>,
    ) -> Self {
        Self {
            label: label.into(),
            to: Some(to),
            query,
        }
    }
    pub fn current(label: impl Into<AttrValue>) -> Self {
        Self {
            label: label.into(),
            to: None,
            query: Vec::new(),
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct PageHeaderProps {
    #[prop_or_default]
    pub crumbs: Vec<Crumb>,
    pub title: AttrValue,
    #[prop_or_default]
    pub subtitle: Option<AttrValue>,
    /// Page-level actions aligned to the end of the title row.
    #[prop_or_default]
    pub children: Html,
}

#[function_component(PageHeader)]
pub fn page_header(props: &PageHeaderProps) -> Html {
    let breadcrumb = if props.crumbs.is_empty() {
        html! {}
    } else {
        let last = props.crumbs.len() - 1;
        let items = props.crumbs.iter().enumerate().map(|(i, crumb)| {
            let sep = (i < last).then(|| html! { <span class="breadcrumb__sep">{ "/" }</span> });
            let item = match &crumb.to {
                Some(route) => html! {
                    <Link<Route, Vec<(String, String)>>
                        to={route.clone()}
                        query={(!crumb.query.is_empty()).then(|| crumb.query.clone())}
                        classes="breadcrumb__item">
                        { crumb.label.clone() }
                    </Link<Route, Vec<(String, String)>>>
                },
                None => html! {
                    <span class="breadcrumb__item breadcrumb__item--current">{ crumb.label.clone() }</span>
                },
            };
            html! { <>{ item }{ sep }</> }
        }).collect::<Html>();
        html! { <nav class="breadcrumb">{ items }</nav> }
    };

    let subtitle = props
        .subtitle
        .as_ref()
        .map(|s| html! { <p class="page-header__subtitle">{ s.clone() }</p> });

    let actions = (props.children != Html::default())
        .then(|| html! { <div class="page-header__actions">{ props.children.clone() }</div> });

    html! {
        <>
            { breadcrumb }
            <div class="page-header">
                <div class="page-header__text">
                    <h1 class="page-header__title">{ props.title.clone() }</h1>
                    { subtitle }
                </div>
                { actions }
            </div>
        </>
    }
}
