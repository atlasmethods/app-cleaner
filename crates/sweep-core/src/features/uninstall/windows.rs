//! Windows "Programs and Features": the `Uninstall` registry keys.
//!
//! Reading the registry sits behind [`UninstallRegistry`] so parsing, filtering and the
//! uninstall-string handling are unit-tested on any host with a fake; the real
//! implementation (`winreg`) is compiled on Windows only.

use std::collections::HashMap;

use crate::error::Result;
use crate::features::uninstall::model::{Action, Found, Hive, Source, WinAction};
use crate::pkgutil::date_from_yyyymmdd;

#[derive(Debug, Clone, PartialEq)]
pub enum RegValue {
    Str(String),
    Dword(u32),
}

/// One subkey of an `Uninstall` key with its values.
#[derive(Debug, Clone, PartialEq)]
pub struct RegEntry {
    pub hive: Hive,
    pub key_name: String,
    pub values: HashMap<String, RegValue>,
}

impl RegEntry {
    pub fn new(hive: Hive, key_name: &str) -> RegEntry {
        RegEntry {
            hive,
            key_name: key_name.to_string(),
            values: HashMap::new(),
        }
    }
    pub fn with_str(mut self, k: &str, v: &str) -> RegEntry {
        self.values
            .insert(k.to_string(), RegValue::Str(v.to_string()));
        self
    }
    pub fn with_dword(mut self, k: &str, v: u32) -> RegEntry {
        self.values.insert(k.to_string(), RegValue::Dword(v));
        self
    }
    fn string(&self, k: &str) -> Option<String> {
        match self.values.get(k) {
            Some(RegValue::Str(s)) => {
                let t = s.trim();
                (!t.is_empty()).then(|| t.to_string())
            }
            Some(RegValue::Dword(n)) => Some(n.to_string()),
            None => None,
        }
    }
    fn dword(&self, k: &str) -> Option<u32> {
        match self.values.get(k) {
            Some(RegValue::Dword(n)) => Some(*n),
            Some(RegValue::Str(s)) => s.trim().parse().ok(),
            None => None,
        }
    }
    fn flag(&self, k: &str) -> bool {
        self.dword(k) == Some(1)
    }
}

pub trait UninstallRegistry {
    /// Every subkey of the three `Uninstall` keys (HKLM 64-bit, HKLM WOW6432Node, HKCU).
    fn entries(&self) -> Result<Vec<RegEntry>>;
}

/// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`
pub fn is_guid_key(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 38 || b[0] != b'{' || b[37] != b'}' {
        return false;
    }
    s[1..37].char_indices().all(|(i, c)| match i {
        8 | 13 | 18 | 23 => c == '-',
        _ => c.is_ascii_hexdigit(),
    })
}

const UPDATE_RELEASE_TYPES: &[&str] = &[
    "update",
    "hotfix",
    "security update",
    "update rollup",
    "service pack",
    "critical update",
];

/// Turn raw registry entries into list entries, dropping updates, child entries and
/// nameless keys.
pub fn parse_entries(entries: &[RegEntry]) -> Vec<Found> {
    let mut v = Vec::new();
    for e in entries {
        let Some(name) = e.string("DisplayName") else {
            continue;
        };
        // Patches and components of a parent product are not separate programs.
        if e.string("ParentKeyName").is_some() || e.string("ParentDisplayName").is_some() {
            continue;
        }
        if let Some(rt) = e.string("ReleaseType") {
            if UPDATE_RELEASE_TYPES.contains(&rt.to_lowercase().as_str()) {
                continue;
            }
        }
        if e.key_name.is_empty() || e.key_name.contains(['\\', '/']) {
            continue;
        }
        let uninstall = e.string("UninstallString");
        let quiet = e.string("QuietUninstallString");
        let modify = e.string("ModifyPath");
        let is_guid = is_guid_key(&e.key_name);
        let msi = is_guid
            && (e.flag("WindowsInstaller")
                || uninstall
                    .as_deref()
                    .is_some_and(|u| u.to_lowercase().contains("msiexec")));
        let mut f = Found::new(
            format!("windows:{}:{}", e.hive.tag(), e.key_name),
            name,
            e.string("DisplayVersion").unwrap_or_default(),
            Source::Windows,
            Action::Windows(WinAction {
                hive: e.hive,
                key_name: e.key_name.clone(),
                uninstall_string: uninstall.clone(),
                quiet_uninstall_string: quiet.clone(),
                modify_path: modify.clone(),
                msi_product_code: msi.then(|| e.key_name.clone()),
            }),
        );
        f.entry.publisher = e.string("Publisher").unwrap_or_default();
        f.entry.install_date = e.string("InstallDate").and_then(|d| date_from_yyyymmdd(&d));
        f.entry.size_bytes = e
            .dword("EstimatedSize")
            .filter(|kb| *kb > 0)
            .map(|kb| kb as u64 * 1024);
        f.entry.uninstallable = !e.flag("NoRemove") && (uninstall.is_some() || quiet.is_some());
        f.entry.can_repair = !e.flag("NoRepair") && (msi || modify.is_some());
        f.entry.can_modify = !e.flag("NoModify") && (modify.is_some() || msi);
        f.entry.is_system = e.flag("SystemComponent");
        f.entry.icon = e
            .string("DisplayIcon")
            .map(|i| i.rsplit_once(',').map(|(p, _)| p.to_string()).unwrap_or(i))
            .map(|i| i.trim_matches('"').to_string());
        v.push(f);
    }
    v
}

// ---------------------------------------------------------------- command lines

/// Expand `%NAME%` references (REG_EXPAND_SZ values are stored unexpanded).
pub fn expand_env(s: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) if j > 0 => match lookup(&after[..j]) {
                Some(val) => {
                    out.push_str(&val);
                    rest = &after[j + 1..];
                }
                None => {
                    out.push('%');
                    rest = after;
                }
            },
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Split arguments the way `CommandLineToArgvW` does for the common cases: whitespace
/// separates, double quotes group (and are removed).
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut started = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                started = true;
            }
            c if c.is_whitespace() && !in_q => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

/// Split a registry command line into program and arguments.
///
/// - `"C:\Program Files\App\unins000.exe" /SILENT` (quoted program)
/// - `MsiExec.exe /X{GUID}`
/// - `C:\Program Files\App\uninst.exe /S` (unquoted path containing spaces: the program
///   ends at the first `.exe` / `.com` / `.bat` / `.cmd` boundary, as `CreateProcess` guesses)
pub fn parse_command_line(cmd: &str) -> Option<(String, Vec<String>)> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return None;
    }
    if let Some(rest) = cmd.strip_prefix('"') {
        let end = rest.find('"')?;
        let prog = rest[..end].trim().to_string();
        if prog.is_empty() {
            return None;
        }
        return Some((prog, split_args(&rest[end + 1..])));
    }
    let lower = cmd.to_ascii_lowercase();
    for ext in [".exe", ".com", ".bat", ".cmd"] {
        let mut from = 0;
        while let Some(i) = lower[from..].find(ext) {
            let end = from + i + ext.len();
            // The extension must end the token (followed by whitespace or the end).
            let boundary = match lower[end..].chars().next() {
                None => true,
                Some(c) => c.is_whitespace(),
            };
            if boundary {
                return Some((cmd[..end].to_string(), split_args(&cmd[end..])));
            }
            from = end;
        }
    }
    let mut it = split_args(cmd).into_iter();
    let prog = it.next()?;
    Some((prog, it.collect()))
}

/// `MsiExec.exe /I{GUID}` means "install/modify"; uninstalling needs `/X{GUID}`.
pub fn msi_install_to_uninstall(args: &mut [String]) {
    for a in args.iter_mut() {
        let lower = a.to_lowercase();
        if let Some(rest) = lower.strip_prefix("/i") {
            if rest.starts_with('{') {
                *a = format!("/X{}", &a[2..]);
            }
        }
    }
}

/// Program + args to run to uninstall `w` (quiet variant preferred).
pub fn uninstall_command(w: &WinAction) -> Option<(String, Vec<String>)> {
    let raw = w
        .quiet_uninstall_string
        .as_deref()
        .or(w.uninstall_string.as_deref())?;
    let raw = expand_env(raw, |n| std::env::var(n).ok());
    let (prog, mut args) = parse_command_line(&raw)?;
    let base = prog
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(&prog)
        .to_lowercase();
    if base == "msiexec" || base == "msiexec.exe" {
        msi_install_to_uninstall(&mut args);
    }
    Some((prog, args))
}

/// Program + args to repair `w`: `msiexec /fa {GUID}` for Windows Installer products,
/// otherwise its `ModifyPath`.
pub fn repair_command(w: &WinAction) -> Option<(String, Vec<String>)> {
    if let Some(code) = &w.msi_product_code {
        return Some(("msiexec".into(), vec!["/fa".into(), code.clone()]));
    }
    let raw = expand_env(w.modify_path.as_deref()?, |n| std::env::var(n).ok());
    parse_command_line(&raw)
}

/// Characters we refuse in registry key names / display names we pass to `reg.exe`.
pub fn reg_arg_ok(s: &str) -> bool {
    !s.is_empty()
        && !s
            .chars()
            .any(|c| matches!(c, '"' | '%' | '\r' | '\n' | '\0') || c.is_control())
}

/// Sanity check for key names used to build `reg` paths.
pub fn key_name_ok(s: &str) -> bool {
    // Key names are anything without backslashes; reject option-like leading characters.
    reg_arg_ok(s) && !s.contains('\\') && !s.starts_with(['-', '/'])
}

// ---------------------------------------------------------------- real registry

#[cfg(windows)]
pub struct RealRegistry;

#[cfg(windows)]
impl UninstallRegistry for RealRegistry {
    fn entries(&self) -> Result<Vec<RegEntry>> {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
        use winreg::types::FromRegValue;
        use winreg::RegKey;

        const TAIL: &str = r"Microsoft\Windows\CurrentVersion\Uninstall";
        let mut out = Vec::new();
        let roots = [
            (
                HKEY_LOCAL_MACHINE,
                format!(r"SOFTWARE\{TAIL}"),
                Hive::Hklm64,
            ),
            (
                HKEY_LOCAL_MACHINE,
                format!(r"SOFTWARE\WOW6432Node\{TAIL}"),
                Hive::Hklm32,
            ),
            (HKEY_CURRENT_USER, format!(r"SOFTWARE\{TAIL}"), Hive::Hkcu),
        ];
        for (root, path, hive) in roots {
            let Ok(base) = RegKey::predef(root).open_subkey_with_flags(&path, KEY_READ) else {
                continue;
            };
            for name in base.enum_keys().filter_map(|k| k.ok()) {
                let Ok(key) = base.open_subkey_with_flags(&name, KEY_READ) else {
                    continue;
                };
                let mut entry = RegEntry::new(hive, &name);
                for r in key.enum_values() {
                    let Ok((vname, raw)) = r else { continue };
                    use winreg::enums::{REG_DWORD, REG_EXPAND_SZ, REG_SZ};
                    let v = match raw.vtype {
                        REG_SZ | REG_EXPAND_SZ => {
                            String::from_reg_value(&raw).ok().map(RegValue::Str)
                        }
                        REG_DWORD => u32::from_reg_value(&raw).ok().map(RegValue::Dword),
                        _ => None,
                    };
                    if let Some(v) = v {
                        entry.values.insert(vname, v);
                    }
                }
                out.push(entry);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUID: &str = "{90160000-008C-0000-0000-0000000FF1CE}";

    fn firefox() -> RegEntry {
        RegEntry::new(Hive::Hklm64, "Mozilla Firefox 126.0 (x64 en-US)")
            .with_str("DisplayName", "Mozilla Firefox (x64 en-US)")
            .with_str("DisplayVersion", "126.0")
            .with_str("Publisher", "Mozilla")
            .with_str("InstallDate", "20240520")
            .with_dword("EstimatedSize", 226_000)
            .with_str(
                "UninstallString",
                r#""C:\Program Files\Mozilla Firefox\uninstall\helper.exe""#,
            )
            .with_str(
                "QuietUninstallString",
                r#""C:\Program Files\Mozilla Firefox\uninstall\helper.exe" /S"#,
            )
            .with_str(
                "DisplayIcon",
                r#""C:\Program Files\Mozilla Firefox\firefox.exe",0"#,
            )
            .with_dword("NoModify", 1)
            .with_dword("NoRepair", 1)
    }

    fn office() -> RegEntry {
        RegEntry::new(Hive::Hklm32, GUID)
            .with_str("DisplayName", "Microsoft Office Professional Plus 2010")
            .with_str("DisplayVersion", "14.0.7015.1000")
            .with_str("Publisher", "Microsoft Corporation")
            .with_str("UninstallString", &format!("MsiExec.exe /I{GUID}"))
            .with_dword("WindowsInstaller", 1)
            .with_dword("EstimatedSize", 0)
    }

    #[test]
    fn parses_a_typical_exe_installer() {
        let v = parse_entries(&[firefox()]);
        assert_eq!(v.len(), 1);
        let e = &v[0].entry;
        assert_eq!(e.id, "windows:hklm64:Mozilla Firefox 126.0 (x64 en-US)");
        assert_eq!(e.name, "Mozilla Firefox (x64 en-US)");
        assert_eq!(e.version, "126.0");
        assert_eq!(e.publisher, "Mozilla");
        assert_eq!(e.install_date.as_deref(), Some("2024-05-20"));
        assert_eq!(e.size_bytes, Some(226_000 * 1024));
        assert!(e.uninstallable && !e.can_repair && !e.can_modify && !e.is_system);
        assert_eq!(
            e.icon.as_deref(),
            Some(r"C:\Program Files\Mozilla Firefox\firefox.exe")
        );
    }

    #[test]
    fn msi_products_can_repair_and_modify() {
        let v = parse_entries(&[office()]);
        let e = &v[0].entry;
        assert!(e.uninstallable && e.can_repair && e.can_modify);
        assert_eq!(e.size_bytes, None);
        let Action::Windows(w) = &v[0].action else {
            panic!()
        };
        assert_eq!(w.msi_product_code.as_deref(), Some(GUID));
        assert_eq!(w.hive, Hive::Hklm32);
        assert_eq!(e.id, format!("windows:hklm32:{GUID}"));
    }

    #[test]
    fn filters_updates_children_nameless_and_marks_system_components() {
        let entries = vec![
            RegEntry::new(Hive::Hklm64, "KB5034441")
                .with_str("DisplayName", "Security Update for Windows (KB5034441)")
                .with_str("ReleaseType", "Security Update"),
            RegEntry::new(Hive::Hklm64, "Hotfix1")
                .with_str("DisplayName", "Hotfix for X")
                .with_str("ReleaseType", "Hotfix"),
            RegEntry::new(Hive::Hklm64, "child")
                .with_str("DisplayName", "Child")
                .with_str("ParentKeyName", "OperatingSystem"),
            RegEntry::new(Hive::Hklm64, "nameless").with_str("UninstallString", "x.exe"),
            RegEntry::new(Hive::Hklm64, "blank").with_str("DisplayName", "   "),
            RegEntry::new(Hive::Hklm64, "sys")
                .with_str("DisplayName", "Windows Driver Package - Foo")
                .with_dword("SystemComponent", 1)
                .with_str("UninstallString", "rundll32.exe x"),
            RegEntry::new(Hive::Hkcu, "NoUninstall")
                .with_str("DisplayName", "Portable-ish")
                .with_dword("NoRemove", 1)
                .with_str("UninstallString", "u.exe"),
            RegEntry::new(Hive::Hkcu, "NoString").with_str("DisplayName", "No Uninstall String"),
            RegEntry::new(Hive::Hkcu, "bad\\key").with_str("DisplayName", "Evil"),
        ];
        let v = parse_entries(&entries);
        let names: Vec<&str> = v.iter().map(|f| f.entry.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Windows Driver Package - Foo",
                "Portable-ish",
                "No Uninstall String"
            ]
        );
        assert!(v[0].entry.is_system);
        assert!(!v[1].entry.uninstallable);
        assert!(!v[2].entry.uninstallable);
        assert!(parse_entries(&[]).is_empty());
    }

    #[test]
    fn dword_typed_strings_and_bad_dates_are_tolerated() {
        let e = RegEntry::new(Hive::Hkcu, "x")
            .with_str("DisplayName", "X")
            .with_str("InstallDate", "not a date")
            .with_str("EstimatedSize", "2048")
            .with_str("UninstallString", "x.exe");
        let v = parse_entries(&[e]);
        assert_eq!(v[0].entry.install_date, None);
        assert_eq!(v[0].entry.size_bytes, Some(2048 * 1024));
    }

    #[test]
    fn guid_keys() {
        assert!(is_guid_key(GUID));
        assert!(!is_guid_key("{not-a-guid}"));
        assert!(!is_guid_key("90160000-008C-0000-0000-0000000FF1CE"));
        assert!(!is_guid_key("{90160000-008C-0000-0000-0000000FF1CG}"));
    }

    #[test]
    fn command_line_parsing() {
        assert_eq!(
            parse_command_line(r#""C:\Program Files\App\unins000.exe" /SILENT /NORESTART"#),
            Some((
                r"C:\Program Files\App\unins000.exe".into(),
                vec!["/SILENT".into(), "/NORESTART".into()]
            ))
        );
        assert_eq!(
            parse_command_line("MsiExec.exe /X{90160000-008C-0000-0000-0000000FF1CE}"),
            Some((
                "MsiExec.exe".into(),
                vec!["/X{90160000-008C-0000-0000-0000000FF1CE}".into()]
            ))
        );
        assert_eq!(
            parse_command_line(
                r"C:\Program Files\App Dir\uninst.exe /S /D=C:\Program Files\App Dir"
            ),
            Some((
                r"C:\Program Files\App Dir\uninst.exe".into(),
                vec![
                    "/S".into(),
                    "/D=C:\\Program".into(),
                    "Files\\App".into(),
                    "Dir".into()
                ]
            ))
        );
        assert_eq!(
            parse_command_line(r#""C:\x\y.exe""#),
            Some((r"C:\x\y.exe".into(), vec![]))
        );
        // quoted argument with spaces stays one argument
        assert_eq!(
            parse_command_line(r#"C:\a\b.exe --dir "C:\Some Dir" -q"#),
            Some((
                r"C:\a\b.exe".into(),
                vec!["--dir".into(), r"C:\Some Dir".into(), "-q".into()]
            ))
        );
        // ".exe" inside a longer token is not a boundary
        assert_eq!(
            parse_command_line(r"C:\a\my.exe.config\run.exe /q"),
            Some((r"C:\a\my.exe.config\run.exe".into(), vec!["/q".into()]))
        );
        assert_eq!(
            parse_command_line(
                "rundll32.exe dfshim.dll,ShArpMaintain App.application, Culture=neutral"
            ),
            Some((
                "rundll32.exe".into(),
                vec![
                    "dfshim.dll,ShArpMaintain".into(),
                    "App.application,".into(),
                    "Culture=neutral".into()
                ]
            ))
        );
        assert_eq!(parse_command_line("   "), None);
        assert_eq!(parse_command_line(r#""unterminated"#), None);
        assert_eq!(parse_command_line(r#""" /x"#), None);
        // no known extension: first token is the program
        assert_eq!(
            parse_command_line("uninstaller -q"),
            Some(("uninstaller".into(), vec!["-q".into()]))
        );
    }

    #[test]
    fn env_expansion() {
        let look = |n: &str| (n == "ProgramFiles").then(|| r"C:\Program Files".to_string());
        assert_eq!(
            expand_env(r"%ProgramFiles%\App\u.exe /S", look),
            r"C:\Program Files\App\u.exe /S"
        );
        assert_eq!(expand_env("100%", look), "100%");
        assert_eq!(expand_env("%UNKNOWN%\\x", look), "%UNKNOWN%\\x");
        assert_eq!(expand_env("a%%b", look), "a%%b");
    }

    #[test]
    fn uninstall_command_prefers_quiet_and_fixes_msi_install_string() {
        let v = parse_entries(&[firefox(), office()]);
        let Action::Windows(ff) = &v[0].action else {
            panic!()
        };
        assert_eq!(
            uninstall_command(ff),
            Some((
                r"C:\Program Files\Mozilla Firefox\uninstall\helper.exe".into(),
                vec!["/S".into()]
            ))
        );
        let Action::Windows(o) = &v[1].action else {
            panic!()
        };
        assert_eq!(
            uninstall_command(o),
            Some(("MsiExec.exe".into(), vec![format!("/X{GUID}")]))
        );
        let none = WinAction {
            hive: Hive::Hkcu,
            key_name: "k".into(),
            uninstall_string: None,
            quiet_uninstall_string: None,
            modify_path: None,
            msi_product_code: None,
        };
        assert_eq!(uninstall_command(&none), None);
    }

    #[test]
    fn msi_switch_normalisation() {
        let mut a = vec![
            "/i{GUID}".to_string(),
            "/qn".to_string(),
            "/X{G}".to_string(),
        ];
        msi_install_to_uninstall(&mut a);
        assert_eq!(a, ["/X{GUID}", "/qn", "/X{G}"]);
        // "/install" style switches are not touched
        let mut b = vec!["/install".to_string()];
        msi_install_to_uninstall(&mut b);
        assert_eq!(b, ["/install"]);
    }

    #[test]
    fn repair_commands() {
        let v = parse_entries(&[office()]);
        let Action::Windows(o) = &v[0].action else {
            panic!()
        };
        assert_eq!(
            repair_command(o),
            Some(("msiexec".into(), vec!["/fa".into(), GUID.into()]))
        );
        let w = WinAction {
            hive: Hive::Hklm64,
            key_name: "k".into(),
            uninstall_string: None,
            quiet_uninstall_string: None,
            modify_path: Some(r#""C:\App\setup.exe" /repair"#.into()),
            msi_product_code: None,
        };
        assert_eq!(
            repair_command(&w),
            Some((r"C:\App\setup.exe".into(), vec!["/repair".into()]))
        );
    }

    #[test]
    fn reg_paths() {
        assert_eq!(
            Hive::Hklm32.reg_path("K"),
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\K"
        );
        assert_eq!(Hive::from_tag("hkcu"), Some(Hive::Hkcu));
        assert_eq!(Hive::from_tag("x"), None);
        assert!(!reg_arg_ok("a\"b"));
        assert!(!reg_arg_ok("50%"));
        assert!(reg_arg_ok("Mozilla Firefox (x64 en-US)"));
        assert!(!key_name_ok("a\\b"));
        assert!(!key_name_ok("-v"));
    }
}
