//! Registry / config issues: scan for and repair broken settings (Windows registry, stale config elsewhere).
//!
//! Planned methods:
//! - `registry_cleaner.scan`
//! - `registry_cleaner.fix`
//! - `registry_cleaner.list_backups`
//! - `registry_cleaner.restore_backup`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "registry_cleaner.scan",
    "registry_cleaner.fix",
    "registry_cleaner.list_backups",
    "registry_cleaner.restore_backup",
];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
