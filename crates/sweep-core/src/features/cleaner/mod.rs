//! Cleaner: rule-driven analysis and removal of temp files, caches, logs and browser data.
//!
//! Planned methods:
//! - `cleaner.list_rules`
//! - `cleaner.analyze`
//! - `cleaner.clean`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["cleaner.list_rules", "cleaner.analyze", "cleaner.clean"];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
