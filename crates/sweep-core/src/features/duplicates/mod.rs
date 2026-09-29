//! Duplicate finder: locate duplicate files by size and content hash.
//!
//! Planned methods:
//! - `duplicates.scan`
//! - `duplicates.delete`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["duplicates.scan", "duplicates.delete"];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
