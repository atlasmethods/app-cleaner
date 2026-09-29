//! Secure delete: irrecoverably overwrite and remove files.
//!
//! Planned methods:
//! - `secure_delete.delete`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["secure_delete.delete"];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
