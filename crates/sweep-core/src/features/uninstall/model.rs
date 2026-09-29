//! Wire and internal types of the uninstall feature.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Dpkg,
    Rpm,
    Pacman,
    Flatpak,
    Snap,
    Appimage,
    Windows,
    Macapp,
    Brew,
}

/// One installed application as shown in the list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEntry {
    /// Stable id: `<kind>:<key>`, e.g. `dpkg:firefox`, `flatpak:org.gimp.GIMP`,
    /// `windows:hklm64:{GUID}`, `macapp:/Applications/Foo.app`, `brew-cask:firefox`.
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    /// `YYYY-MM-DD` when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    pub source: Source,
    pub uninstallable: bool,
    pub can_repair: bool,
    pub can_modify: bool,
    /// Essential / required packages, Windows `SystemComponent=1`, Apple apps. Hidden by
    /// default in the UI and refused by `uninstall.run` unless `force` is set.
    pub is_system: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Which registry hive/view an Uninstall key lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    /// `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`
    Hklm64,
    /// `HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall`
    Hklm32,
    /// `HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`
    Hkcu,
}

impl Hive {
    pub fn tag(self) -> &'static str {
        match self {
            Hive::Hklm64 => "hklm64",
            Hive::Hklm32 => "hklm32",
            Hive::Hkcu => "hkcu",
        }
    }
    pub fn from_tag(s: &str) -> Option<Hive> {
        match s {
            "hklm64" => Some(Hive::Hklm64),
            "hklm32" => Some(Hive::Hklm32),
            "hkcu" => Some(Hive::Hkcu),
            _ => None,
        }
    }
    /// Full path as understood by `reg.exe`.
    pub fn reg_path(self, key: &str) -> String {
        let base = match self {
            Hive::Hklm64 => r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            Hive::Hklm32 => r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
            Hive::Hkcu => r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        };
        format!(r"{base}\{key}")
    }
    pub fn needs_admin(self) -> bool {
        !matches!(self, Hive::Hkcu)
    }
}

/// What has to be done to remove/repair an entry (never sent to the client).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Dpkg(String),
    Rpm(String),
    Pacman(String),
    Flatpak(String),
    Snap(String),
    AppImage(PathBuf),
    Windows(WinAction),
    MacApp(PathBuf),
    Brew { name: String, cask: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct WinAction {
    pub hive: Hive,
    pub key_name: String,
    pub uninstall_string: Option<String>,
    pub quiet_uninstall_string: Option<String>,
    pub modify_path: Option<String>,
    /// `{GUID}` when the key is a Windows Installer product code.
    pub msi_product_code: Option<String>,
}

/// A listed application together with the private data needed to act on it.
#[derive(Debug, Clone)]
pub struct Found {
    pub entry: AppEntry,
    pub action: Action,
}

impl Found {
    pub fn new(id: String, name: String, version: String, source: Source, action: Action) -> Found {
        Found {
            entry: AppEntry {
                id,
                name,
                version,
                publisher: String::new(),
                install_date: None,
                size_bytes: None,
                source,
                uninstallable: true,
                can_repair: false,
                can_modify: false,
                is_system: false,
                icon: None,
            },
            action,
        }
    }
}

/// A folder or file that an uninstalled application left behind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Leftover {
    pub path: String,
    pub size_bytes: u64,
    /// `config` | `cache` | `data` | `launcher` | `prefs` | `logs`
    pub kind: String,
}

/// Outcome of `uninstall.run` / `uninstall.repair`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    pub id: String,
    pub name: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub message: String,
    /// Other packages the package manager would remove together with this one; when
    /// non-empty and `force` was not set nothing has been executed.
    #[serde(default)]
    pub also_removes: Vec<String>,
    #[serde(default)]
    pub needs_force: bool,
    #[serde(default)]
    pub reboot_required: bool,
    /// Leftover scan after a successful uninstall.
    #[serde(default)]
    pub leftovers: Vec<Leftover>,
    /// macOS bundle identifier of the removed app (needed to find its preferences).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
}
