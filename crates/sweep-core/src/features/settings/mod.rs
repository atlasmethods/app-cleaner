//! Settings: persistent app settings stored in the data dir.
//!
//! Planned methods:
//! - `settings.get`
//! - `settings.set`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["settings.get", "settings.set"];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
