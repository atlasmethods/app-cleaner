//! Performance optimizer: identify and put background apps to sleep, tune resource usage.
//!
//! Planned methods:
//! - `optimizer.analyze`
//! - `optimizer.apply`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["optimizer.analyze", "optimizer.apply"];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
