//! Re-group stored exceptions when the fingerprinting rules change.

use tracing_batteries::prelude::*;

use crate::errors::Result;
use crate::store::Store;

use super::exception::{FINGERPRINT_VERSION, fingerprint};

/// Re-group every stored exception when the grouping rules have changed since the
/// data was last processed, then stamp the current rules version. A no-op when the
/// stored version already matches [`FINGERPRINT_VERSION`] (the common path), so it
/// is cheap to call unconditionally at start-up.
///
/// Recomputes each occurrence's `exc_group` from its stored `(type, message,
/// stack)` using the current rules, so live and historical occurrences of the
/// same failure land in one group. The work scales with the number of distinct
/// stored errors, so it runs to completion before the server begins accepting
/// traffic.
pub fn regroup_if_needed(store: &Store) -> Result<()> {
    let applied = store.fingerprint_version()?;
    if applied == FINGERPRINT_VERSION {
        return Ok(());
    }

    info!(
        "exception grouping rules changed (applied v{applied}, current v{FINGERPRINT_VERSION}); \
         re-grouping stored exceptions"
    );

    let remap = |exc_type: &str, message: Option<&str>, stack: Option<&str>| {
        fingerprint(exc_type, message.unwrap_or_default(), stack, None)
    };
    let changed = store.regroup_exceptions(&remap)?;
    store.set_fingerprint_version(FINGERPRINT_VERSION)?;

    info!("re-grouped {changed} exception occurrences to grouping rules v{FINGERPRINT_VERSION}");
    Ok(())
}
