//! Crate-wide error handling built on [`human_errors`], so every failure surfaced
//! to an operator carries actionable advice.

pub use human_errors::{Error, ResultExt};

/// Convenient alias for results that fail with a [`human_errors::Error`].
pub type Result<T> = std::result::Result<T, Error>;
