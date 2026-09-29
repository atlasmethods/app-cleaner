//! Scheduler: run cleans on a schedule.
//!
//! Planned methods:
//! - `scheduler.list`
//! - `scheduler.add`
//! - `scheduler.remove`
//! - `scheduler.set_enabled`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "scheduler.list",
    "scheduler.add",
    "scheduler.remove",
    "scheduler.set_enabled",
];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
