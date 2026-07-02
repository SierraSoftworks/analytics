//! Shared API contract for the analytics service.
//!
//! This crate is deliberately free of any web-framework, database, or UI
//! dependencies so that it can be compiled both by the native `analytics`
//! server and by the WebAssembly `analytics-ui` frontend.

mod auth;
mod exception;
mod health;
mod instance;
mod pixel;
mod project;
mod source;
mod stats;
mod track;

pub use auth::{AdminUser, CsrfToken};
pub use exception::{
    ExceptionBreakdowns, ExceptionGroup, ExceptionGroupDetail, ExceptionReport, ExceptionStatus,
    ExceptionVariant, GlobalException, TREND_BUCKETS, TriageInput,
};
pub use health::Health;
pub use instance::Instance;
pub use pixel::{Pixel, PixelInput};
pub use project::{Project, ProjectInput};
pub use source::{
    Source, SourceInput, SourceKind, SourceScheme, app_source, default_kind, pixel_id_of,
    pixel_source, source_label, source_scheme, website_source,
};
pub use stats::{
    BreakdownRow, Breakdowns, CountRow, Dashboard, DashboardQuery, MetricSummary, TimeSeriesPoint,
};
pub use track::{BeaconKind, TrackEvent};
