//! Event ingest: enrichment (UA/language/geo/referrer/UTM), bot filtering, and the
//! non-blocking batched writer + compactor pipeline.

mod compactor;
mod enrich;
mod exception;
mod geo;
mod language;
mod pipeline;
mod referrer;
mod ua;

pub use enrich::build_event;
pub use exception::build_exception;
pub use pipeline::{Ingest, spawn};
