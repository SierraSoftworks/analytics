//! Shared API contract for the analytics service.
//!
//! This crate is deliberately free of any web-framework, database, or UI
//! dependencies so that it can be compiled both by the native `analytics`
//! server and by the WebAssembly `analytics-ui` frontend.

mod auth;
mod health;

pub use auth::{AdminUser, CsrfToken};
pub use health::Health;
