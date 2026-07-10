//! Event ingest: enrichment (UA/language/geo/referrer/UTM), bot filtering, and the
//! non-blocking batched writer + compactor pipeline.

mod compactor;
mod enrich;
mod exception;
mod geo;
mod language;
mod normalize;
mod pipeline;
mod referrer;
mod ua;

pub use enrich::build_event;
pub use exception::build_exception;
pub use pipeline::{Ingest, spawn};

/// Truncate `value` to at most `max` bytes on a char boundary, appending an ellipsis
/// when shortened. Shared by the hit and exception ingest paths so every stored text
/// field has a bounded size regardless of client input.
fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}
