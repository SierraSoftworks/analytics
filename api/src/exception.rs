use serde::{Deserialize, Serialize};

/// The triage status of an exception group, set by an administrator. Stored
/// separately from the (append-only) occurrence stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExceptionStatus {
    #[default]
    Unresolved,
    Resolved,
    Ignored,
}
