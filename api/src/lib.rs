//! Shared API contract for the analytics service.
//!
//! This crate is deliberately free of any web-framework, database, or UI
//! dependencies so that it can be compiled both by the native `analytics`
//! server and by the WebAssembly `analytics-ui` frontend.

mod auth;
mod exception;
mod health;
mod pixel;
mod project;
mod source;
mod stats;
mod track;

pub use auth::{AdminUser, CsrfToken};
pub use exception::ExceptionStatus;
pub use health::Health;
pub use pixel::{Pixel, PixelInput};
pub use project::{Project, ProjectInput};
pub use source::{
    Source, SourceInput, SourceKind, SourceScheme, app_source, default_kind, pixel_id_of,
    pixel_source, source_label, source_scheme, website_source,
};
pub use stats::{KeyCount, MetricSummary, Overview, ProjectSummary, Stats, TimeSeriesPoint};
pub use track::{BeaconKind, PingResponse, TrackEvent};
