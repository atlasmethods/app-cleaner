//! Driver updater: detect outdated drivers and update them.
//!
//! Planned methods:
//! - `driver_updater.scan`
//! - `driver_updater.update`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["driver_updater.scan", "driver_updater.update"];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
