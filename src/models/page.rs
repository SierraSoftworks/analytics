use crate::api::APIError;
use actix::prelude::*;
use tracing_batteries::prelude::*;

#[derive(Clone)]
pub struct Page {
    pub domain: String,
    pub path: String,
    pub views: u64,
    pub likes: u64,
}

actor_message!(GetPages(domain: String) -> Vec<Page>);
actor_message!(GetPage(domain: String, path: String) -> Page);
actor_message!(ViewPage(domain: String, path: String) -> Page);
actor_message!(LikePage(domain: String, path: String) -> Page);

#[derive(Serialize, Deserialize)]
pub struct PageV1 {
    pub domain: String,
    pub path: String,
    pub views: u64,
    pub likes: u64,
}

json_responder!(PageV1);

impl From<Page> for PageV1 {
    fn from(state: Page) -> Self {
        Self {
            domain: state.domain,
            path: state.path,
            views: state.views,
            likes: state.likes,
        }
    }
}

impl From<PageV1> for Page {
    fn from(val: PageV1) -> Self {
        Page {
            domain: val.domain,
            path: val.path,
            views: val.views,
            likes: val.likes,
        }
    }
}
