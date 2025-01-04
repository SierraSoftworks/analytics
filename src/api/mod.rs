#[macro_use]
mod macros;

mod embed;
mod error;
mod health;
mod likes;
mod pages;
mod views;

#[cfg(test)]
pub mod test;

use actix_web::web;

pub use error::APIError;

pub fn configure(cfg: &mut web::ServiceConfig) {
    embed::configure(cfg);
    health::configure(cfg);
    likes::configure(cfg);
    pages::configure(cfg);
    views::configure(cfg);
}
