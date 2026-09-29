//! Software updater: detect outdated applications and update them via the platform package manager.
//!
//! Planned methods:
//! - `software_updater.list`
//! - `software_updater.update`
//! - `software_updater.update_all`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "software_updater.list",
    "software_updater.update",
    "software_updater.update_all",
];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
