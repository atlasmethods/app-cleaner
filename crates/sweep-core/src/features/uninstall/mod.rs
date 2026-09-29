//! Uninstall: list installed applications and remove them.
//!
//! Planned methods:
//! - `uninstall.list`
//! - `uninstall.run`
//! - `uninstall.remove_entry`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["uninstall.list", "uninstall.run", "uninstall.remove_entry"];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
