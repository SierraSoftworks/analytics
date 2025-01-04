use crate::api::APIError;
use crate::{models::*, trace_handler};
use actix::prelude::*;
use std::sync::RwLock;
use std::{collections::BTreeMap, sync::Arc};

pub struct MemoryStore {
    started_at: chrono::DateTime<chrono::Utc>,
    pages: Arc<RwLock<BTreeMap<String, BTreeMap<String, Page>>>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            started_at: chrono::Utc::now(),
            pages: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }
}

impl Actor for MemoryStore {
    type Context = Context<Self>;
}

trace_handler!(MemoryStore, GetHealth, Result<Health, APIError>);

impl Handler<GetHealth> for MemoryStore {
    type Result = Result<Health, APIError>;

    fn handle(&mut self, _: GetHealth, _: &mut Self::Context) -> Self::Result {
        Ok(Health {
            ok: true,
            started_at: self.started_at,
        })
    }
}

trace_handler!(MemoryStore, GetPages, Result<Vec<Page>, APIError>);

impl Handler<GetPages> for MemoryStore {
    type Result = Result<Vec<Page>, APIError>;

    fn handle(&mut self, msg: GetPages, _: &mut Self::Context) -> Self::Result {
        let ps = self.pages.read().map_err(|_| {
            APIError::new(
                500,
                "Internal Server Error",
                "The service is currently unavailable, please try again later.",
            )
        })?;

        Ok(ps
            .get(&msg.domain)
            .map(|pages| pages.values().cloned().collect())
            .unwrap_or_default())
    }
}

trace_handler!(MemoryStore, GetPage, Result<Page, APIError>);

impl Handler<GetPage> for MemoryStore {
    type Result = Result<Page, APIError>;

    fn handle(&mut self, msg: GetPage, _: &mut Self::Context) -> Self::Result {
        let ps = self.pages.read().map_err(|_| {
            APIError::new(
                500,
                "Internal Server Error",
                "The service is currently unavailable, please try again later.",
            )
        })?;

        ps.get(&msg.domain)
            .map(|pages| {
                pages.get(&msg.path).cloned().ok_or_else(|| {
                    APIError::new(
                        404,
                        "Not Found",
                        "The page you requested could not be found.",
                    )
                })
            })
            .unwrap_or_else(|| {
                Err(APIError::new(
                    404,
                    "Not Found",
                    "The page you requested could not be found.",
                ))
            })
    }
}

trace_handler!(MemoryStore, LikePage, Result<Page, APIError>);

impl Handler<LikePage> for MemoryStore {
    type Result = Result<Page, APIError>;

    fn handle(&mut self, msg: LikePage, _: &mut Self::Context) -> Self::Result {
        let mut ps = self.pages.write().map_err(|_| {
            APIError::new(
                500,
                "Internal Server Error",
                "The service is currently unavailable, please try again later.",
            )
        })?;

        let pages = ps.entry(msg.domain.clone()).or_insert_with(BTreeMap::new);

        Ok(pages
            .entry(msg.path.clone())
            .and_modify(|p| p.likes += 1)
            .or_insert_with(|| Page {
                domain: msg.domain,
                path: msg.path,
                likes: 1,
                views: 1,
            })
            .clone())
    }
}

trace_handler!(MemoryStore, ViewPage, Result<Page, APIError>);

impl Handler<ViewPage> for MemoryStore {
    type Result = Result<Page, APIError>;

    fn handle(&mut self, msg: ViewPage, _: &mut Self::Context) -> Self::Result {
        let mut ps = self.pages.write().map_err(|_| {
            APIError::new(
                500,
                "Internal Server Error",
                "The service is currently unavailable, please try again later.",
            )
        })?;

        let pages = ps.entry(msg.domain.clone()).or_insert_with(BTreeMap::new);

        Ok(pages
            .entry(msg.path.clone())
            .and_modify(|p| p.views += 1)
            .or_insert_with(|| Page {
                domain: msg.domain,
                path: msg.path,
                likes: 0,
                views: 1,
            })
            .clone())
    }
}

// trace_handler!(MemoryStore, GetIdea, Result<Idea, APIError>);

// impl Handler<GetIdea> for MemoryStore {
//     type Result = Result<Idea, APIError>;

//     fn handle(&mut self, msg: GetIdea, _: &mut Self::Context) -> Self::Result {
//         let is = self.ideas.read().map_err(|_| {
//             APIError::new(
//                 500,
//                 "Internal Server Error",
//                 "The service is currently unavailable, please try again later.",
//             )
//         })?;

//         is.get(&msg.collection)
//             .ok_or_else(|| APIError::new(404, "Not Found", "The collection ID you provided could not be found. Please check it and try again."))
//             .and_then(|c|
//                 c.get(&msg.id).cloned()
//                 .ok_or_else(|| APIError::new(404, "Not Found", "The idea ID you provided could not be found. Please check it and try again.")))
//     }
// }

// trace_handler!(MemoryStore, GetIdeas, Result<Vec<Idea>, APIError>);

// impl Handler<GetIdeas> for MemoryStore {
//     type Result = Result<Vec<Idea>, APIError>;

//     fn handle(&mut self, msg: GetIdeas, _: &mut Self::Context) -> Self::Result {
//         let is = self.ideas.read().map_err(|_| {
//             APIError::new(
//                 500,
//                 "Internal Server Error",
//                 "The service is currently unavailable, please try again later.",
//             )
//         })?;

//         is.get(&msg.collection)
//             .ok_or_else(|| APIError::new(404, "Not Found", "The collection ID you provided could not be found. Please check it and try again."))
//             .map(|items| items.iter().filter(|(_, i)| {
//                 if let Some(is_completed) = msg.is_completed {
//                     if i.completed != is_completed {
//                         return false;
//                     }
//                 }

//                 if let Some(tag) = msg.tag.clone() {
//                     if !i.tags.contains(tag.as_str()) {
//                         return false;
//                     }
//                 }

//                 true
//             }).map(|(_id, idea)| idea.clone()).collect())
//     }
// }

// trace_handler!(MemoryStore, GetRandomIdea, Result<Idea, APIError>);

// impl Handler<GetRandomIdea> for MemoryStore {
//     type Result = Result<Idea, APIError>;

//     fn handle(&mut self, msg: GetRandomIdea, _: &mut Self::Context) -> Self::Result {
//         let is = self.ideas.read().map_err(|_| {
//             APIError::new(
//                 500,
//                 "Internal Server Error",
//                 "The service is currently unavailable, please try again later.",
//             )
//         })?;

//         is.get(&msg.collection)
//             .ok_or_else(|| APIError::new(404, "Not Found", "The collection ID you provided could not be found. Please check it and try again."))
//             .and_then(|items| items.iter().filter(|(_, i)| {
//                 if let Some(is_completed) = msg.is_completed {
//                     if i.completed != is_completed {
//                         return false;
//                     }
//                 }

//                 if let Some(tag) = msg.tag.clone() {
//                     if !i.tags.contains(tag.as_str()) {
//                         return false;
//                     }
//                 }

//                 true
//             }).choose(&mut rand::thread_rng())
//                 .ok_or_else(|| APIError::new(404, "Not Found", "No random ideas were available."))
//                 .map(|(_id, idea)| idea.clone()))
//     }
// }

// trace_handler!(MemoryStore, StoreIdea, Result<Idea, APIError>);

// impl Handler<StoreIdea> for MemoryStore {
//     type Result = Result<Idea, APIError>;

//     fn handle(&mut self, msg: StoreIdea, _: &mut Self::Context) -> Self::Result {
//         let mut is = self.ideas.write().map_err(|_| {
//             APIError::new(
//                 500,
//                 "Internal Server Error",
//                 "The service is currently unavailable, please try again later.",
//             )
//         })?;

//         let idea = Idea {
//             id: msg.id,
//             collection_id: msg.collection,
//             name: msg.name.clone(),
//             description: msg.description.clone(),
//             tags: msg.tags.clone(),
//             completed: msg.completed,
//         };

//         is.entry(msg.collection)
//             .or_insert_with(BTreeMap::new)
//             .insert(idea.id, idea.clone());

//         Ok(idea)
//     }
// }

// trace_handler!(MemoryStore, RemoveIdea, Result<(), APIError>);

// impl Handler<RemoveIdea> for MemoryStore {
//     type Result = Result<(), APIError>;

//     fn handle(&mut self, msg: RemoveIdea, _: &mut Self::Context) -> Self::Result {
//         let mut is = self.ideas.write().map_err(|_| {
//             APIError::new(
//                 500,
//                 "Internal Server Error",
//                 "The service is currently unavailable, please try again later.",
//             )
//         })?;

//         is.get_mut(&msg.collection)
//             .ok_or_else(|| APIError::new(404, "Not Found", "The collection ID you provided could not be found. Please check it and try again."))
//             .and_then(|c|
//                 c.remove(&msg.id)
//                 .map(|_| ())
//                 .ok_or_else(|| APIError::new(404, "Not Found", "The idea ID you provided could not be found. Please check it and try again.")))
//     }
// }
