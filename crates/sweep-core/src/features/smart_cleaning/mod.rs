//! Smart cleaning: background monitor that prompts or auto-cleans past a junk threshold.
//!
//! Planned methods:
//! - `smart_cleaning.get_config`
//! - `smart_cleaning.set_config`
//! - `smart_cleaning.check`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "smart_cleaning.get_config",
    "smart_cleaning.set_config",
    "smart_cleaning.check",
];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
