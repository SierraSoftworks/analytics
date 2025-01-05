use crate::api::APIError;
use crate::{models::*, trace_handler};
use actix::prelude::*;

pub struct SqliteStore {
    started_at: chrono::DateTime<chrono::Utc>,
    connection: sqlite::Connection,
}

impl SqliteStore {
    pub fn new<P: AsRef<str>>(path: P) -> Result<Self, sqlite::Error> {
        let instance = Self {
            started_at: chrono::Utc::now(),
            connection: sqlite::Connection::open(path.as_ref())?,
        };

        super::migrations::run_migrations(&instance.connection)?;

        Ok(instance)
    }

    fn get_page(&self, domain: &str, path: &str) -> Result<Page, APIError> {
        let mut query = self.connection.prepare(
            "SELECT domain, path, likes, views FROM pages WHERE domain = ? AND path = ? LIMIT 1",
        )?;
        query.bind((1, domain))?;
        query.bind((2, path))?;

        if let Ok(sqlite::State::Row) = query.next() {
            Ok(Page {
                domain: query.read("domain")?,
                path: query.read("path")?,
                likes: query.read::<i64, _>("likes")? as u64,
                views: query.read::<i64, _>("views")? as u64,
            })
        } else {
            Err(APIError::new(
                404,
                "Not Found",
                "The page you requested could not be found in the database.",
            ))
        }
    }

    fn upsert_page(
        &self,
        domain: &str,
        path: &str,
        likes: u32,
        views: u32,
    ) -> Result<Page, APIError> {
        let mut query = self.connection.prepare(format!(
            "INSERT INTO pages (domain, path, likes, views)
              VALUES (?, ?, {likes}, {views})
              ON CONFLICT DO UPDATE
                SET likes = likes + {likes}, views = views + {views}
              RETURNING likes, views"
        ))?;
        query.bind((1, domain))?;
        query.bind((2, path))?;

        if let Ok(sqlite::State::Row) = query.next() {
            Ok(Page {
                domain: domain.to_string(),
                path: path.to_string(),
                likes: query.read::<i64, _>("likes")? as u64,
                views: query.read::<i64, _>("views")? as u64,
            })
        } else {
            Err(APIError::new(
                500,
                "Internal Server Error",
                "An error occurred while updating the page in the database.",
            ))
        }
    }
}

impl Actor for SqliteStore {
    type Context = Context<Self>;
}

trace_handler!(SqliteStore, GetHealth, Result<Health, APIError>);

impl Handler<GetHealth> for SqliteStore {
    type Result = Result<Health, APIError>;

    fn handle(&mut self, _: GetHealth, _: &mut Self::Context) -> Self::Result {
        Ok(Health {
            ok: true,
            started_at: self.started_at,
        })
    }
}

trace_handler!(SqliteStore, GetPages, Result<Vec<Page>, APIError>);

impl Handler<GetPages> for SqliteStore {
    type Result = Result<Vec<Page>, APIError>;

    fn handle(&mut self, msg: GetPages, _: &mut Self::Context) -> Self::Result {
        let mut query = self
            .connection
            .prepare("SELECT domain, path, likes, views FROM pages WHERE domain = ?")?;
        query.bind((1, msg.domain.as_str()))?;

        let mut pages = Vec::new();
        while let Ok(sqlite::State::Row) = query.next() {
            pages.push(Page {
                domain: query.read("domain")?,
                path: query.read("path")?,
                likes: query.read::<i64, _>("likes")? as u64,
                views: query.read::<i64, _>("views")? as u64,
            })
        }

        Ok(pages)
    }
}

trace_handler!(SqliteStore, GetPage, Result<Page, APIError>);

impl Handler<GetPage> for SqliteStore {
    type Result = Result<Page, APIError>;

    fn handle(&mut self, msg: GetPage, _: &mut Self::Context) -> Self::Result {
        self.get_page(&msg.domain, &msg.path)
    }
}

trace_handler!(SqliteStore, ViewPage, Result<Page, APIError>);

impl Handler<ViewPage> for SqliteStore {
    type Result = Result<Page, APIError>;

    fn handle(&mut self, msg: ViewPage, _: &mut Self::Context) -> Self::Result {
        self.upsert_page(&msg.domain, &msg.path, 0, 1)
    }
}

trace_handler!(SqliteStore, LikePage, Result<Page, APIError>);

impl Handler<LikePage> for SqliteStore {
    type Result = Result<Page, APIError>;

    fn handle(&mut self, msg: LikePage, _: &mut Self::Context) -> Self::Result {
        self.upsert_page(&msg.domain, &msg.path, 1, 0)
    }
}
