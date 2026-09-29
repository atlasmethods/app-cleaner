//! Shared data types: issues, categories and the (server-side only) fix actions.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::regaccess::{reg_path, Root};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    Medium,
}

/// One problem found by a scan, as sent to the client.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// Stable hash of category + location + value + data: the same problem has the same id in
    /// every scan, which is how `fix` matches what the user selected against a fresh scan.
    pub id: String,
    pub category: String,
    pub description: String,
    /// Registry key path or file path.
    pub location: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    pub severity: Severity,
    /// The fix needs administrator rights.
    pub needs_admin: bool,
}

/// What `fix` does for one issue. Never leaves the server: the client only names issue ids.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    RegDeleteValue {
        root: Root,
        key: String,
        /// Empty = the key's default value.
        name: String,
    },
    RegDeleteKey {
        root: Root,
        key: String,
    },
    /// Remove a file or symlink (plus related symlinks). `system` files need elevation.
    RemoveFile {
        path: PathBuf,
        also: Vec<PathBuf>,
        system: bool,
    },
    /// Remove one desktop id from one `mime=` line of a `mimeapps.list`.
    MimeRemove {
        file: PathBuf,
        section: String,
        mime: String,
        desktop_id: String,
    },
    /// Remove an orphaned package through the system package manager.
    RemovePackage {
        manager: PkgManager,
        package: String,
        version: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PkgManager {
    Apt,
    Dnf,
    Pacman,
}

impl PkgManager {
    pub fn name(self) -> &'static str {
        match self {
            PkgManager::Apt => "apt",
            PkgManager::Dnf => "dnf",
            PkgManager::Pacman => "pacman",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub issue: Issue,
    pub action: Action,
}

/// A scannable category.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub severity: Severity,
    /// Pre-ticked in the UI.
    pub default_selected: bool,
}

const fn cat(
    id: &'static str,
    label: &'static str,
    description: &'static str,
    severity: Severity,
    default_selected: bool,
) -> CategoryInfo {
    CategoryInfo {
        id,
        label,
        description,
        severity,
        default_selected,
    }
}

pub const WINDOWS_CATEGORIES: &[CategoryInfo] = &[
    cat("shared_dlls", "Missing shared DLLs", "Shared libraries whose file no longer exists", Severity::Low, true),
    cat("file_extensions", "Unused file extensions", "File types that point to nothing", Severity::Low, true),
    cat("activex", "ActiveX and COM", "Components whose program file is gone", Severity::Low, true),
    cat("type_libs", "Type libraries", "Type libraries whose file is gone", Severity::Low, true),
    cat("applications", "Applications", "Program entries whose executable is gone", Severity::Low, true),
    cat("fonts", "Fonts", "Font entries whose font file is gone", Severity::Low, true),
    cat("app_paths", "Application paths", "Program locations that no longer exist", Severity::Low, true),
    cat("help_files", "Help files", "Help entries whose file is gone", Severity::Low, true),
    cat("installer", "Installer", "Installer folders that no longer exist", Severity::Low, true),
    cat("obsolete_software", "Obsolete software", "Uninstall entries of programs that are gone", Severity::Medium, false),
    cat("startup", "Startup entries", "Programs set to start with Windows that are gone", Severity::Low, true),
    cat("menu_order", "Start menu ordering", "Sort order of Start menu folders that no longer exist", Severity::Low, true),
    cat("mui_cache", "MUI cache", "Cached names of programs that are gone", Severity::Low, true),
    cat("sound_events", "Sound events", "Sounds assigned to files that are gone", Severity::Low, true),
    cat("services", "Windows services", "Services whose program is gone", Severity::Medium, false),
];

pub const LINUX_CATEGORIES: &[CategoryInfo] = &[
    cat("desktop_entries", "Broken launchers", "Application launchers whose program is gone", Severity::Low, true),
    cat("autostart", "Broken autostart entries", "Autostart entries whose program is gone", Severity::Low, true),
    cat("broken_symlinks", "Broken links", "Symbolic links pointing to nothing", Severity::Low, true),
    cat("mime_associations", "File associations", "Default applications that are no longer installed", Severity::Low, true),
    cat("user_services", "Stale user services", "systemd user services whose program is gone", Severity::Low, true),
    cat("orphaned_packages", "Orphaned packages", "Packages nothing depends on any more", Severity::Medium, false),
];

pub const MACOS_CATEGORIES: &[CategoryInfo] = &[
    cat("launch_agents", "Broken launch agents", "Launch agents whose program is gone", Severity::Low, true),
    cat("broken_symlinks", "Broken links", "Symbolic links pointing to nothing", Severity::Low, true),
];

pub fn categories_for(os: crate::ctx::Os) -> &'static [CategoryInfo] {
    match os {
        crate::ctx::Os::Windows => WINDOWS_CATEGORIES,
        crate::ctx::Os::Linux => LINUX_CATEGORIES,
        crate::ctx::Os::MacOs => MACOS_CATEGORIES,
    }
}

/// Two independent 64-bit FNV-1a hashes over the parts (separated by an impossible byte),
/// as 32 hex characters.
pub fn issue_id(parts: &[&str]) -> String {
    fn fnv(parts: &[&str], seed: u64) -> u64 {
        let mut h = seed;
        for p in parts {
            for b in p.bytes().chain(std::iter::once(0xff)) {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        h
    }
    format!(
        "{:016x}{:016x}",
        fnv(parts, 0xcbf2_9ce4_8422_2325),
        fnv(parts, 0x84222325cbf29ce4)
    )
}

impl Found {
    /// A registry issue.
    #[allow(clippy::too_many_arguments)]
    pub fn registry(
        category: &str,
        description: String,
        root: Root,
        key: &str,
        value: Option<&str>,
        data: Option<String>,
        severity: Severity,
        action: Action,
    ) -> Found {
        let location = reg_path(root, key);
        let value = value.map(|v| {
            if v.is_empty() {
                "(Default)".to_string()
            } else {
                v.to_string()
            }
        });
        let id = issue_id(&[
            category,
            &location,
            value.as_deref().unwrap_or(""),
            data.as_deref().unwrap_or(""),
        ]);
        Found {
            issue: Issue {
                id,
                category: category.to_string(),
                description,
                location,
                value,
                data,
                severity,
                needs_admin: root.needs_admin(),
            },
            action,
        }
    }

    /// A file-system issue.
    pub fn file(
        category: &str,
        description: String,
        location: &std::path::Path,
        value: Option<String>,
        data: Option<String>,
        severity: Severity,
        needs_admin: bool,
        action: Action,
    ) -> Found {
        let location = location.to_string_lossy().into_owned();
        let id = issue_id(&[
            category,
            &location,
            value.as_deref().unwrap_or(""),
            data.as_deref().unwrap_or(""),
        ]);
        Found {
            issue: Issue {
                id,
                category: category.to_string(),
                description,
                location,
                value,
                data,
                severity,
                needs_admin,
            },
            action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_distinct() {
        let a = issue_id(&["c", "l", "v", ""]);
        assert_eq!(a, issue_id(&["c", "l", "v", ""]));
        assert_eq!(a.len(), 32);
        assert_ne!(a, issue_id(&["c", "l", "v", "d"]));
        assert_ne!(a, issue_id(&["c", "l", "", "v"]));
        // the part boundary matters
        assert_ne!(issue_id(&["ab", "c"]), issue_id(&["a", "bc"]));
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn category_ids_are_unique_per_os() {
        for list in [WINDOWS_CATEGORIES, LINUX_CATEGORIES, MACOS_CATEGORIES] {
            let mut ids: Vec<_> = list.iter().map(|c| c.id).collect();
            ids.sort_unstable();
            let n = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), n);
        }
        assert_eq!(WINDOWS_CATEGORIES.len(), 15);
    }
}
