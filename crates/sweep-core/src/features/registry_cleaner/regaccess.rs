//! The Windows registry and file system as seen by the registry scanners.
//!
//! Everything the scanners need from Windows sits behind [`WinProbe`] (files, environment,
//! drives) and [`RegistryAccess`] (keys and values), so every category is unit-tested on any
//! host with a fake (`fake.rs`). The real implementation ([`RealRegistry`], `winreg`) is
//! compiled on Windows only.
//!
//! Design rule: a probe never guesses. Anything that is not clearly "does not exist" is
//! reported as [`Stat::Unknown`] (access denied, locked volume, ...), and the scanners never
//! report an item whose state is unknown.

use serde::{Deserialize, Serialize};

/// The two hives ClearSweep touches. `HKEY_CLASSES_ROOT` is the merged view of
/// `HKLM\SOFTWARE\Classes` and `HKCU\SOFTWARE\Classes`; both halves are scanned separately so
/// every issue names the hive that really holds it (and knows whether it needs elevation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Root {
    Hklm,
    Hkcu,
}

impl Root {
    pub fn short(self) -> &'static str {
        match self {
            Root::Hklm => "HKLM",
            Root::Hkcu => "HKCU",
        }
    }
    /// The name used inside `.reg` files.
    pub fn long(self) -> &'static str {
        match self {
            Root::Hklm => "HKEY_LOCAL_MACHINE",
            Root::Hkcu => "HKEY_CURRENT_USER",
        }
    }
    pub fn needs_admin(self) -> bool {
        self == Root::Hklm
    }
}

/// `HKLM\SOFTWARE\Foo` for `(Hklm, "SOFTWARE\Foo")`.
pub fn reg_path(root: Root, key: &str) -> String {
    format!("{}\\{}", root.short(), key)
}

/// Inverse of [`reg_path`].
pub fn split_reg_path(path: &str) -> Option<(Root, &str)> {
    let (head, rest) = path.split_once('\\')?;
    let root = match head.to_ascii_uppercase().as_str() {
        "HKLM" | "HKEY_LOCAL_MACHINE" => Root::Hklm,
        "HKCU" | "HKEY_CURRENT_USER" => Root::Hkcu,
        _ => return None,
    };
    (!rest.is_empty()).then_some((root, rest))
}

/// What a file system probe found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stat {
    File,
    Dir,
    /// Definitely not there.
    Missing,
    /// Could not tell (access denied, volume locked, ...). Never treated as missing.
    Unknown,
}

/// File system and environment queries.
pub trait WinProbe {
    fn stat(&self, path: &str) -> Stat;
    fn env_var(&self, name: &str) -> Option<String>;
    /// Is `letter:` a local fixed disk? Removable, optical and network drives come and go, so a
    /// file on them is never called missing.
    fn is_fixed_drive(&self, letter: char) -> bool;
}

#[derive(Debug, Clone, PartialEq)]
pub enum RegData {
    Str(String),
    ExpandStr(String),
    Dword(u32),
    /// Any other type (binary, multi-string, ...): never interpreted.
    Other,
}

impl RegData {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            RegData::Str(s) | RegData::ExpandStr(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_dword(&self) -> Option<u32> {
        match self {
            RegData::Dword(n) => Some(*n),
            RegData::Str(s) | RegData::ExpandStr(s) => s.trim().parse().ok(),
            RegData::Other => None,
        }
    }
    /// Short text for the issue list.
    pub fn display(&self) -> String {
        match self {
            RegData::Str(s) | RegData::ExpandStr(s) => s.clone(),
            RegData::Dword(n) => n.to_string(),
            RegData::Other => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegValue {
    /// Empty for the key's default value.
    pub name: String,
    pub data: RegData,
}

/// Read-only registry access. A key that does not exist (or cannot be opened) yields empty
/// results: unreadable means "not reported".
pub trait RegistryAccess: WinProbe {
    fn subkeys(&self, root: Root, key: &str) -> Vec<String>;
    fn values(&self, root: Root, key: &str) -> Vec<RegValue>;
    fn key_exists(&self, root: Root, key: &str) -> bool;

    /// The string value `name` ("" = default) of `key`.
    fn string(&self, root: Root, key: &str, name: &str) -> Option<String> {
        self.values(root, key)
            .into_iter()
            .find(|v| v.name.eq_ignore_ascii_case(name))
            .and_then(|v| v.data.as_str().map(str::to_string))
    }
}

// ---------------------------------------------------------------- real Windows

#[cfg(windows)]
pub struct RealRegistry;

#[cfg(windows)]
mod real {
    use super::*;
    use winreg::enums::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, REG_DWORD,
        REG_EXPAND_SZ, REG_SZ,
    };
    use winreg::types::FromRegValue;
    use winreg::RegKey;

    fn open(root: Root, key: &str) -> Option<RegKey> {
        let predef = match root {
            Root::Hklm => HKEY_LOCAL_MACHINE,
            Root::Hkcu => HKEY_CURRENT_USER,
        };
        // Always the 64-bit view: the 32-bit views are addressed through explicit
        // `WOW6432Node` paths so that the path in an issue is the path `reg.exe` will delete.
        RegKey::predef(predef)
            .open_subkey_with_flags(key, KEY_READ | KEY_WOW64_64KEY)
            .ok()
    }

    impl WinProbe for RealRegistry {
        fn stat(&self, path: &str) -> Stat {
            match std::fs::metadata(path) {
                Ok(m) if m.is_dir() => Stat::Dir,
                Ok(_) => Stat::File,
                // Only a clean "not found" counts as missing; access denied, sharing
                // violations, locked volumes and the like are "unknown".
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Stat::Missing,
                Err(_) => Stat::Unknown,
            }
        }
        fn env_var(&self, name: &str) -> Option<String> {
            std::env::var(name).ok().filter(|v| !v.is_empty())
        }
        fn is_fixed_drive(&self, letter: char) -> bool {
            use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
            const DRIVE_FIXED: u32 = 3;
            let root: Vec<u16> = format!("{letter}:\\")
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            // SAFETY: `root` is a valid NUL-terminated UTF-16 string that outlives the call.
            unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_FIXED }
        }
    }

    impl RegistryAccess for RealRegistry {
        fn subkeys(&self, root: Root, key: &str) -> Vec<String> {
            open(root, key)
                .map(|k| k.enum_keys().filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        }
        fn values(&self, root: Root, key: &str) -> Vec<RegValue> {
            let Some(k) = open(root, key) else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for r in k.enum_values() {
                let Ok((name, raw)) = r else { continue };
                let data = match raw.vtype {
                    REG_SZ => String::from_reg_value(&raw)
                        .map(RegData::Str)
                        .unwrap_or(RegData::Other),
                    REG_EXPAND_SZ => String::from_reg_value(&raw)
                        .map(RegData::ExpandStr)
                        .unwrap_or(RegData::Other),
                    REG_DWORD => u32::from_reg_value(&raw)
                        .map(RegData::Dword)
                        .unwrap_or(RegData::Other),
                    _ => RegData::Other,
                };
                out.push(RegValue { name, data });
            }
            out
        }
        fn key_exists(&self, root: Root, key: &str) -> bool {
            open(root, key).is_some()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reg_paths_round_trip() {
        assert_eq!(reg_path(Root::Hklm, r"SOFTWARE\A"), r"HKLM\SOFTWARE\A");
        assert_eq!(
            split_reg_path(r"HKCU\Software\B"),
            Some((Root::Hkcu, r"Software\B"))
        );
        assert_eq!(
            split_reg_path(r"HKEY_LOCAL_MACHINE\X"),
            Some((Root::Hklm, "X"))
        );
        assert_eq!(split_reg_path(r"HKCR\X"), None);
        assert_eq!(split_reg_path("HKLM"), None);
        assert_eq!(split_reg_path(r"HKLM\"), None);
    }

    #[test]
    fn data_accessors() {
        assert_eq!(RegData::Str("a".into()).as_str(), Some("a"));
        assert_eq!(RegData::Dword(3).as_str(), None);
        assert_eq!(RegData::Str(" 7 ".into()).as_dword(), Some(7));
        assert_eq!(RegData::Other.as_dword(), None);
        assert_eq!(RegData::Dword(3).display(), "3");
    }
}
