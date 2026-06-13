use serde::{Deserialize, Serialize};

/// Liveness/readiness response returned by the public health endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub version: String,
}
