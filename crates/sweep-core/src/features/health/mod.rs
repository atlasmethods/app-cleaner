//! Health Check: one-click overview of junk, privacy and performance issues.
//!
//! Planned methods:
//! - `health.analyze`
//! - `health.fix`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["health.analyze", "health.fix"];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
