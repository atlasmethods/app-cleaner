//! Startup manager: list, enable, disable and remove startup items.
//!
//! Planned methods:
//! - `startup.list`
//! - `startup.set_enabled`
//! - `startup.remove`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["startup.list", "startup.set_enabled", "startup.remove"];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
