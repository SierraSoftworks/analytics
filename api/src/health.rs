use serde::{Deserialize, Serialize};

/// Liveness/readiness response. Deliberately omits version/build details so the
/// public health endpoint does not disclose server information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
}
