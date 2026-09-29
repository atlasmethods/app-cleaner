//! Cookie manager: list browser cookies and maintain a keep-list.
//!
//! Planned methods:
//! - `cookies.list`
//! - `cookies.get_keep_list`
//! - `cookies.set_keep_list`

use crate::api::Registry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "cookies.list",
    "cookies.get_keep_list",
    "cookies.set_keep_list",
];

pub fn register(r: &mut Registry) {
    r.stubs(METHODS);
}
