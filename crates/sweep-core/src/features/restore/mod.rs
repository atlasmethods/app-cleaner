//! System restore: list, create and roll back to restore points.
//!
//! Planned methods:
//! - `restore.list_points`
//! - `restore.create_point`
//! - `restore.restore`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "restore.list_points",
    "restore.create_point",
    "restore.restore",
];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
