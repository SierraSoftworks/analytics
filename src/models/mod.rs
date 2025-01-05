#[macro_use]
mod macros;

mod health;
mod page;

use actix::prelude::*;

pub use health::*;
pub use page::*;

#[derive(Clone)]
pub struct GlobalState {
    pub store: Addr<crate::store::Store>,
}

impl GlobalState {
    pub fn new<P: AsRef<str>>(database_path: P) -> Result<Self, sqlite::Error> {
        Ok(Self {
            store: crate::store::Store::new(database_path)?.start(),
        })
    }
}
