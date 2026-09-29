//! Disk analyzer: break down disk usage by file type and find the largest files.
//!
//! Planned methods:
//! - `disk_analyzer.scan`
//! - `disk_analyzer.top_files`
//! - `disk_analyzer.delete`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "disk_analyzer.scan",
    "disk_analyzer.top_files",
    "disk_analyzer.delete",
];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
