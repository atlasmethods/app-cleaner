//! Browser plugins: list, enable, disable and remove browser extensions.
//!
//! Planned methods:
//! - `browser_plugins.list`
//! - `browser_plugins.set_enabled`
//! - `browser_plugins.remove`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "browser_plugins.list",
    "browser_plugins.set_enabled",
    "browser_plugins.remove",
];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
