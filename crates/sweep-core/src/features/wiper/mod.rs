//! Drive wiper: overwrite free space or whole drives.
//!
//! Planned methods:
//! - `wiper.list_drives`
//! - `wiper.wipe_free_space`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["wiper.list_drives", "wiper.wipe_free_space"];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
