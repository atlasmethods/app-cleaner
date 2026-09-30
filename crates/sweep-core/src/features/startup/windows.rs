//! Windows startup items: `Run` / `RunOnce` values, startup folders, scheduled tasks,
//! services and shell context-menu handlers.
//!
//! Reads of the registry go through [`StartupRegistry`] (a fake on other hosts, `winreg` on
//! Windows). Everything that changes the system runs `reg.exe`, `schtasks.exe`, `sc.exe` or
//! PowerShell through `ctx.runner` (elevated with `run_privileged` when machine-wide), so the
//! exact commands are unit-tested on any host.
//!
//! Run values and startup-folder shortcuts are disabled the way Task Manager does it: by
//! writing a 12-byte value (`03 00 00 00` + FILETIME) under
//! `...\Explorer\StartupApproved\Run|Run32|StartupFolder`, never by deleting the entry.

use serde_json::Value;
use std::path::Path;

use crate::ctx::Ctx;
use crate::elevate::{powershell_encode, run_privileged};
use crate::error::{ApiError, Result};
use crate::fsutil::now_unix;
use crate::pkgutil::summarize;
use crate::runner::CmdOutput;

use super::impact::exe_from_command;
use super::model::{Entry, Kind, RunLoc, Scope, StartupItem, Target, WinHive};

// ---------------------------------------------------------------- registry access

#[derive(Debug, Clone, PartialEq)]
pub enum RegData {
    Str(String),
    Binary(Vec<u8>),
    Dword(u32),
}

impl RegData {
    pub fn as_string(&self) -> Option<String> {
        match self {
            RegData::Str(s) => Some(s.clone()),
            _ => None,
        }
    }
}

pub trait StartupRegistry {
    /// Named values of a key (empty when the key does not exist).
    fn values(&self, hive: WinHive, key: &str) -> Vec<(String, RegData)>;
    /// Names of the sub-keys.
    fn subkeys(&self, hive: WinHive, key: &str) -> Vec<String>;
    /// The `(Default)` value.
    fn default_value(&self, hive: WinHive, key: &str) -> Option<String>;
    fn key_exists(&self, hive: WinHive, key: &str) -> bool;
}

// ---------------------------------------------------------------- Run keys

pub const RUN: &str = r"Microsoft\Windows\CurrentVersion\Run";
pub const RUN_ONCE: &str = r"Microsoft\Windows\CurrentVersion\RunOnce";
pub const APPROVED: &str = r"Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

/// `(tag, location)` for every Run-style key.
pub fn run_locations() -> Vec<(&'static str, RunLoc)> {
    let l = |hive, key: String, approved| RunLoc {
        hive,
        key,
        approved,
    };
    vec![
        (
            "hkcu-run",
            l(WinHive::Hkcu, format!(r"Software\{RUN}"), Some("Run")),
        ),
        (
            "hkcu-runonce",
            l(WinHive::Hkcu, format!(r"Software\{RUN_ONCE}"), None),
        ),
        (
            "hklm-run",
            l(WinHive::Hklm, format!(r"SOFTWARE\{RUN}"), Some("Run")),
        ),
        (
            "hklm-runonce",
            l(WinHive::Hklm, format!(r"SOFTWARE\{RUN_ONCE}"), None),
        ),
        (
            "hklm-run32",
            l(
                WinHive::Hklm,
                format!(r"SOFTWARE\WOW6432Node\{RUN}"),
                Some("Run32"),
            ),
        ),
        (
            "hklm-runonce32",
            l(
                WinHive::Hklm,
                format!(r"SOFTWARE\WOW6432Node\{RUN_ONCE}"),
                None,
            ),
        ),
    ]
}

pub fn approved_key(hive: WinHive, sub: &str) -> String {
    let root = if hive == WinHive::Hkcu {
        "Software"
    } else {
        "SOFTWARE"
    };
    format!(r"{root}\{APPROVED}\{sub}")
}

/// StartupApproved semantics: first byte even (0x02, 0x06) = enabled, odd (0x03, 0x07) =
/// disabled; no value = enabled.
pub fn approved_enabled(data: &[u8]) -> bool {
    data.first().is_none_or(|b| b & 1 == 0)
}

fn filetime_now() -> u64 {
    (now_unix() + 11_644_473_600) * 10_000_000
}

/// The 12 bytes Task Manager writes: `03 00 00 00` + FILETIME when disabling,
/// `02 00 00 00` + zeros when enabling.
pub fn approved_value(enabled: bool) -> Vec<u8> {
    let mut v = vec![if enabled { 2 } else { 3 }, 0, 0, 0];
    if enabled {
        v.extend_from_slice(&[0u8; 8]);
    } else {
        v.extend_from_slice(&filetime_now().to_le_bytes());
    }
    v
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn lookup_ci<'a>(vals: &'a [(String, RegData)], name: &str) -> Option<&'a RegData> {
    vals.iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, d)| d)
}

fn approved_state(reg: &dyn StartupRegistry, hive: WinHive, sub: &str, name: &str) -> bool {
    let vals = reg.values(hive, &approved_key(hive, sub));
    match lookup_ci(&vals, name) {
        Some(RegData::Binary(b)) => approved_enabled(b),
        _ => true,
    }
}

/// Startup entries that Windows itself relies on.
fn run_is_critical(name: &str, cmd: &str) -> bool {
    let n = name.to_lowercase();
    let c = cmd.to_lowercase();
    n == "securityhealth"
        || n == "windowsdefender"
        || n.starts_with("sechealth")
        || c.contains(r"\windows defender\")
        || c.contains("securityhealthsystray")
        || c.contains(r"\windows\system32\")
}

pub fn publisher_from_path(p: &str) -> Option<String> {
    let l = p.replace('/', r"\");
    let low = l.to_lowercase();
    for marker in [r"\program files (x86)\", r"\program files\"] {
        if let Some(i) = low.find(marker) {
            let rest = &l[i + marker.len()..];
            let vendor = rest.split('\\').next()?.trim();
            if !vendor.is_empty() && !vendor.contains('.') {
                return Some(vendor.to_string());
            }
        }
    }
    None
}

pub fn collect_run(reg: &dyn StartupRegistry) -> Vec<Entry> {
    let mut out = Vec::new();
    for (tag, loc) in run_locations() {
        let user = loc.hive == WinHive::Hkcu;
        for (name, data) in reg.values(loc.hive, &loc.key) {
            if name.is_empty() {
                continue;
            }
            let Some(cmd) = data.as_string() else {
                continue;
            };
            let critical = run_is_critical(&name, &cmd);
            let once = loc.approved.is_none();
            let enabled = match loc.approved {
                Some(sub) => approved_state(reg, loc.hive, sub, &name),
                None => true,
            };
            let mut item = StartupItem::new(
                format!("winrun:{tag}:{name}"),
                name.clone(),
                Kind::Autostart,
                if user { Scope::User } else { Scope::System },
            );
            item.command = cmd.clone();
            item.location = format!(r"{}\{}", loc.hive.name(), loc.key);
            item.enabled = enabled;
            item.critical = critical;
            item.can_disable = !critical && !once;
            item.can_delete = !critical;
            item.publisher = publisher_from_path(&cmd);
            let mut e = Entry::new(
                item,
                Target::WinRun {
                    loc: loc.clone(),
                    name,
                },
            );
            e.exe = exe_from_command(&cmd);
            out.push(e);
        }
    }
    out
}

// ---------------------------------------------------------------- startup folders

/// `.lnk` targets resolved by one PowerShell call: `[{Name, Target, Args}]`.
pub fn lnk_script(dir: &Path) -> String {
    let d = dir.to_string_lossy().replace('\'', "''");
    format!(
        "$s = New-Object -ComObject WScript.Shell; \
         Get-ChildItem -LiteralPath '{d}' -Filter *.lnk -ErrorAction SilentlyContinue | \
         ForEach-Object {{ $l = $s.CreateShortcut($_.FullName); \
         [pscustomobject]@{{ Name = $_.Name; Target = $l.TargetPath; Args = $l.Arguments }} }} | \
         ConvertTo-Json -Compress"
    )
}

/// One JSON object or an array of them (PowerShell's `ConvertTo-Json` collapses singletons).
pub fn json_objects(text: &str) -> Vec<Value> {
    let t = text.trim().trim_start_matches('\u{feff}');
    if t.is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<Value>(t) {
        Ok(Value::Array(a)) => a.into_iter().filter(Value::is_object).collect(),
        Ok(v @ Value::Object(_)) => vec![v],
        _ => Vec::new(),
    }
}

fn s_field(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn startup_folders(ctx: &Ctx) -> Vec<(bool, std::path::PathBuf)> {
    vec![
        (
            false,
            ctx.env
                .config_dir
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup"),
        ),
        (
            true,
            ctx.env
                .sys_path("/ProgramData")
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("StartUp"),
        ),
    ]
}

pub fn collect_folders(ctx: &Ctx, reg: &dyn StartupRegistry) -> Vec<Entry> {
    let mut out = Vec::new();
    for (all_users, dir) in startup_folders(ctx) {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<String> = rd
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.eq_ignore_ascii_case("desktop.ini"))
            .collect();
        files.sort();
        if files.is_empty() {
            continue;
        }
        let mut targets: Vec<Value> = Vec::new();
        if files.iter().any(|f| f.to_lowercase().ends_with(".lnk")) {
            let script = lnk_script(&dir);
            if let Ok(o) = ctx.runner.run(
                "powershell",
                &["-NoProfile", "-NonInteractive", "-Command", &script],
            ) {
                if o.success() {
                    targets = json_objects(&o.stdout);
                }
            }
        }
        let hive = if all_users {
            WinHive::Hklm
        } else {
            WinHive::Hkcu
        };
        for file in files {
            let resolved = targets
                .iter()
                .find(|t| s_field(t, "Name").is_some_and(|n| n.eq_ignore_ascii_case(&file)))
                .and_then(|t| {
                    let tp = s_field(t, "Target")?;
                    Some(match s_field(t, "Args") {
                        Some(a) => format!("\"{tp}\" {a}"),
                        None => tp,
                    })
                });
            let path = dir.join(&file);
            let command = resolved
                .clone()
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            let mut item = StartupItem::new(
                format!(
                    "winfolder:{}:{file}",
                    if all_users { "all" } else { "user" }
                ),
                file.trim_end_matches(".lnk").to_string(),
                Kind::Autostart,
                if all_users {
                    Scope::System
                } else {
                    Scope::User
                },
            );
            item.command = command.clone();
            item.location = path.to_string_lossy().into_owned();
            item.enabled = approved_state(reg, hive, "StartupFolder", &file);
            item.can_delete = true;
            item.publisher = publisher_from_path(&command);
            let mut e = Entry::new(
                item,
                Target::WinFolder {
                    path,
                    file,
                    all_users,
                },
            );
            e.exe = resolved.as_deref().and_then(exe_from_command);
            out.push(e);
        }
    }
    out
}

// ---------------------------------------------------------------- scheduled tasks

/// RFC 4180 CSV records (quoted fields may contain commas, quotes and line breaks).
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    let mut has_any = false;
    while let Some(c) = chars.next() {
        if in_q {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cur.push('"');
                } else {
                    in_q = false;
                }
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' => {
                in_q = true;
                has_any = true;
            }
            ',' => {
                row.push(std::mem::take(&mut cur));
                has_any = true;
            }
            '\r' => {}
            '\n' => {
                if has_any || !cur.is_empty() {
                    row.push(std::mem::take(&mut cur));
                    rows.push(std::mem::take(&mut row));
                }
                has_any = false;
            }
            c => {
                cur.push(c);
                has_any = true;
            }
        }
    }
    if has_any || !cur.is_empty() {
        row.push(cur);
        rows.push(row);
    }
    rows
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// Full name including folder, e.g. `\Vendor\Updater`.
    pub name: String,
    pub command: String,
    pub author: String,
    pub run_as: String,
    pub disabled: bool,
    /// The state text was in a language we do not know.
    pub state_unknown: bool,
}

const COL_TASK_NAME: usize = 1;
const COL_AUTHOR: usize = 7;
const COL_TASK_TO_RUN: usize = 8;
const COL_STATE: usize = 11;
const COL_STATUS: usize = 3;
const COL_RUN_AS: usize = 14;

const DISABLED_WORDS: &[&str] = &[
    "disabled",
    "deaktiviert",
    "désactivé",
    "desactivé",
    "deshabilitado",
    "deshabilitada",
    "desactivado",
    "desactivada",
    "disabilitato",
    "disattivato",
    "uitgeschakeld",
    "wyłączone",
    "wyłączony",
    "отключено",
    "отключена",
    "отключён",
    "無効",
    "已禁用",
    "已停用",
    "禁用",
    "사용 안 함",
    "desativado",
    "inaktiverad",
    "devre dışı",
    "vypnuto",
];
const ENABLED_WORDS: &[&str] = &[
    "enabled",
    "aktiviert",
    "activé",
    "habilitado",
    "habilitada",
    "activado",
    "activada",
    "abilitato",
    "attivato",
    "ingeschakeld",
    "włączone",
    "włączony",
    "включено",
    "включена",
    "有効",
    "已启用",
    "已啟用",
    "启用",
    "사용",
    "ativado",
    "aktiverad",
    "etkin",
    "zapnuto",
];

/// `Some(true)` = disabled, `Some(false)` = enabled, `None` = unrecognised language.
fn state_disabled(text: &str) -> Option<bool> {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return None;
    }
    if DISABLED_WORDS.iter().any(|w| t.contains(w)) {
        return Some(true);
    }
    if ENABLED_WORDS.iter().any(|w| t.contains(w)) {
        return Some(false);
    }
    None
}

/// Parse `schtasks /query /fo csv /v`. The header row is repeated (once per folder), tasks
/// with several triggers span several rows, and the header text is localised, so columns are
/// addressed by position and header rows are recognised by equality with the first row.
pub fn parse_schtasks(csv: &str) -> Vec<Task> {
    let rows = parse_csv(csv);
    let Some(header) = rows.first().cloned() else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for row in rows.iter().skip(1) {
        if *row == header || row.len() <= COL_RUN_AS {
            continue;
        }
        let name = row[COL_TASK_NAME].trim();
        if name.is_empty() || !name.starts_with('\\') || name == row_header_cell(&header) {
            continue;
        }
        if !seen.insert(name.to_string()) {
            continue;
        }
        let state = state_disabled(&row[COL_STATE]).or_else(|| state_disabled(&row[COL_STATUS]));
        out.push(Task {
            name: name.to_string(),
            command: row[COL_TASK_TO_RUN].trim().to_string(),
            author: row[COL_AUTHOR].trim().to_string(),
            run_as: row[COL_RUN_AS].trim().to_string(),
            disabled: state == Some(true),
            state_unknown: state.is_none(),
        });
    }
    out
}

fn row_header_cell(header: &[String]) -> &str {
    header.get(COL_TASK_NAME).map(String::as_str).unwrap_or("")
}

pub fn task_is_critical(name: &str) -> bool {
    name.to_lowercase().starts_with(r"\microsoft\windows\")
}

fn task_system_scope(run_as: &str) -> bool {
    let r = run_as.to_lowercase();
    r.contains("system") || r.contains("service")
}

pub fn collect_tasks(ctx: &Ctx) -> Vec<Entry> {
    let Ok(o) = ctx.runner.run("schtasks", &["/query", "/fo", "csv", "/v"]) else {
        return Vec::new();
    };
    if !o.success() && o.stdout.trim().is_empty() {
        return Vec::new();
    }
    parse_schtasks(&o.stdout)
        .into_iter()
        .map(|t| {
            let critical = task_is_critical(&t.name);
            let short = t.name.rsplit('\\').next().unwrap_or(&t.name).to_string();
            let mut item = StartupItem::new(
                format!("wintask:{}", t.name),
                short,
                Kind::ScheduledTask,
                if task_system_scope(&t.run_as) || critical {
                    Scope::System
                } else {
                    Scope::User
                },
            );
            item.command = t.command.clone();
            item.location = t.name.clone();
            item.enabled = !t.disabled;
            item.critical = critical;
            item.can_disable = !critical;
            item.can_delete = !critical;
            if !t.author.is_empty() && !t.author.eq_ignore_ascii_case("n/a") {
                item.publisher = Some(t.author.clone());
            }
            if t.state_unknown {
                item.warning = Some(
                    "The task state could not be read on this Windows language; it is shown as enabled."
                        .into(),
                );
            }
            let mut e = Entry::new(item, Target::WinTask { name: t.name });
            e.exe = exe_from_command(&t.command);
            e
        })
        .collect()
}

// ---------------------------------------------------------------- services

#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    pub name: String,
    pub display: String,
    pub start_mode: String,
    pub state: String,
    pub path: String,
}

pub const SERVICES_SCRIPT: &str = "Get-CimInstance Win32_Service | \
     Select-Object Name,DisplayName,StartMode,State,PathName | ConvertTo-Json -Compress";

pub fn parse_services(json: &str) -> Vec<Service> {
    json_objects(json)
        .iter()
        .filter_map(|v| {
            Some(Service {
                name: s_field(v, "Name")?,
                display: s_field(v, "DisplayName").unwrap_or_default(),
                start_mode: s_field(v, "StartMode").unwrap_or_default(),
                state: s_field(v, "State").unwrap_or_default(),
                path: s_field(v, "PathName").unwrap_or_default(),
            })
        })
        .collect()
}

pub fn service_is_critical(s: &Service) -> bool {
    let p = s.path.to_lowercase().replace('/', r"\");
    p.is_empty()
        || p.contains(r"\windows\")
        || p.contains(r"\windows defender\")
        || p.contains(r"\windowsapps\")
        || p.contains(r"\windows kits\")
        || p.contains("svchost.exe")
}

pub fn collect_services(ctx: &Ctx) -> Vec<Entry> {
    let Ok(o) = ctx.runner.run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", SERVICES_SCRIPT],
    ) else {
        return Vec::new();
    };
    if !o.success() {
        return Vec::new();
    }
    parse_services(&o.stdout)
        .into_iter()
        .filter_map(|s| {
            let mode = s.start_mode.to_lowercase();
            let enabled = match mode.as_str() {
                "auto" => true,
                "disabled" => false,
                _ => return None,
            };
            let critical = service_is_critical(&s);
            let mut item = StartupItem::new(
                format!("winsvc:{}", s.name),
                if s.display.is_empty() {
                    s.name.clone()
                } else {
                    s.display.clone()
                },
                Kind::Service,
                Scope::System,
            );
            item.command = s.path.clone();
            item.location = format!("Service {}", s.name);
            item.enabled = enabled;
            item.critical = critical;
            item.can_disable = !critical;
            item.publisher = publisher_from_path(&s.path);
            let mut e = Entry::new(item, Target::WinService { name: s.name });
            e.exe = exe_from_command(&s.path);
            Some(e)
        })
        .collect()
}

// ---------------------------------------------------------------- context menu handlers

/// `(tag, path below <Classes>)`
pub const CONTEXT_CLASSES: &[(&str, &str)] = &[
    ("all-files", r"*\shellex\ContextMenuHandlers"),
    ("directory", r"Directory\shellex\ContextMenuHandlers"),
    (
        "directory-background",
        r"Directory\Background\shellex\ContextMenuHandlers",
    ),
    ("folder", r"Folder\shellex\ContextMenuHandlers"),
    (
        "all-fs-objects",
        r"AllFilesystemObjects\shellex\ContextMenuHandlers",
    ),
];

const BUILTIN_HANDLERS: &[&str] = &[
    "open with",
    "openwith",
    "sharing",
    "sendto",
    "send to",
    "new",
    "offline files",
    "library location",
    "pintostartscreen",
    "pintohomefile",
    "previous versions",
    "cast to device",
    "burn",
    "give access to",
    "modern sharing",
    "bitlocker",
    "compatibility",
    "cryptosignmenu",
    "cryptoenc",
    "opencontainingfolder",
    "openas",
    "shellex",
];

fn classes_root(hive: WinHive) -> &'static str {
    if hive == WinHive::Hkcu {
        r"Software\Classes"
    } else {
        r"SOFTWARE\Classes"
    }
}

pub fn is_guid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 38 && b[0] == b'{' && b[37] == b'}'
}

fn expand_sysroot(p: &str, ctx: &Ctx) -> String {
    let root = ctx.env.sys_path("/Windows").to_string_lossy().into_owned();
    p.replace("%SystemRoot%", &root).replace("%windir%", &root)
}

pub fn collect_context(ctx: &Ctx, reg: &dyn StartupRegistry) -> Vec<Entry> {
    let mut out = Vec::new();
    for hive in [WinHive::Hkcu, WinHive::Hklm] {
        for (tag, sub) in CONTEXT_CLASSES {
            let key = format!(r"{}\{sub}", classes_root(hive));
            for handler in reg.subkeys(hive, &key) {
                let disabled = handler.starts_with('-');
                let base = handler.trim_start_matches('-').to_string();
                if base.is_empty() {
                    continue;
                }
                let hkey = format!(r"{key}\{handler}");
                let clsid = reg
                    .default_value(hive, &hkey)
                    .map(|s| s.trim().to_string())
                    .filter(|s| is_guid(s))
                    .or_else(|| is_guid(&base).then(|| base.clone()));
                let mut dll = None;
                let mut friendly = None;
                if let Some(c) = &clsid {
                    'find: for h in [WinHive::Hkcu, WinHive::Hklm] {
                        let root = classes_root(h);
                        for prefix in [
                            format!(r"{root}\CLSID"),
                            format!(r"{root}\WOW6432Node\CLSID"),
                        ] {
                            let ck = format!(r"{prefix}\{c}");
                            if friendly.is_none() {
                                friendly =
                                    reg.default_value(h, &ck).filter(|s| !s.trim().is_empty());
                            }
                            if let Some(d) = reg.default_value(h, &format!(r"{ck}\InprocServer32"))
                            {
                                dll = Some(expand_sysroot(d.trim(), ctx));
                                break 'find;
                            }
                        }
                    }
                }
                let low_dll = dll
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .replace('/', r"\");
                let builtin = low_dll.contains(r"\windows\")
                    || BUILTIN_HANDLERS.contains(&base.to_lowercase().as_str());
                if builtin {
                    continue;
                }
                let name = friendly.unwrap_or_else(|| base.clone());
                let user = hive == WinHive::Hkcu;
                let mut item = StartupItem::new(
                    format!("winctx:{}:{tag}:{base}", hive.name().to_lowercase()),
                    name,
                    Kind::ContextMenu,
                    if user { Scope::User } else { Scope::System },
                );
                item.command = dll
                    .clone()
                    .unwrap_or_else(|| clsid.clone().unwrap_or_default());
                item.location = format!(r"{}\{hkey}", hive.name());
                item.enabled = !disabled;
                item.can_delete = true;
                item.publisher = dll.as_deref().and_then(publisher_from_path);
                let mut e = Entry::new(
                    item,
                    Target::WinContext {
                        hive,
                        key: key.clone(),
                        handler,
                    },
                );
                e.exe = dll;
                out.push(e);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- acting

/// Arguments that end up in a `reg` / `schtasks` / PowerShell command line: no quotes,
/// percent signs or control characters.
pub fn arg_ok(s: &str) -> bool {
    !s.is_empty()
        && !s
            .chars()
            .any(|c| matches!(c, '"' | '%' | '\r' | '\n' | '\0') || c.is_control())
}

fn ensure_ok(out: CmdOutput, what: &str) -> Result<()> {
    if out.success() {
        Ok(())
    } else {
        Err(ApiError::io(format!("{what} failed: {}", summarize(&out))))
    }
}

fn reg_run(ctx: &Ctx, hive: WinHive, args: &[&str]) -> Result<CmdOutput> {
    if hive.needs_admin() {
        run_privileged(ctx, "reg", args)
    } else {
        ctx.runner.run("reg", args)
    }
}

pub fn write_approved(
    ctx: &Ctx,
    hive: WinHive,
    sub: &str,
    name: &str,
    enabled: bool,
) -> Result<()> {
    if !arg_ok(name) {
        return Err(ApiError::invalid_params(
            "the entry name contains characters that cannot be changed safely",
        ));
    }
    let key = format!(r"{}\{}", hive.name(), approved_key(hive, sub));
    let data = hex(&approved_value(enabled));
    let out = reg_run(
        ctx,
        hive,
        &[
            "add",
            &key,
            "/v",
            name,
            "/t",
            "REG_BINARY",
            "/d",
            &data,
            "/f",
        ],
    )?;
    ensure_ok(out, "reg add")
}

pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn powershell(ctx: &Ctx, hive: WinHive, script: &str) -> Result<CmdOutput> {
    let enc = powershell_encode(script);
    let args = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-EncodedCommand",
        enc.as_str(),
    ];
    if hive.needs_admin() {
        run_privileged(ctx, "powershell", &args)
    } else {
        ctx.runner.run("powershell", &args)
    }
}

pub fn rename_handler_script(hive: WinHive, key: &str, from: &str, to: &str) -> String {
    let root = if hive == WinHive::Hkcu {
        "HKEY_CURRENT_USER"
    } else {
        "HKEY_LOCAL_MACHINE"
    };
    format!(
        "$ErrorActionPreference = 'Stop'\nRename-Item -LiteralPath {} -NewName {}\n",
        ps_quote(&format!(r"Registry::{root}\{key}\{from}")),
        ps_quote(to)
    )
}

pub fn set_enabled(ctx: &Ctx, target: &Target, enabled: bool) -> Result<()> {
    match target {
        Target::WinRun { loc, name } => {
            let Some(sub) = loc.approved else {
                return Err(ApiError::unsupported(
                    "RunOnce entries run once; delete the entry instead",
                ));
            };
            write_approved(ctx, loc.hive, sub, name, enabled)
        }
        Target::WinFolder {
            file, all_users, ..
        } => {
            let hive = if *all_users {
                WinHive::Hklm
            } else {
                WinHive::Hkcu
            };
            write_approved(ctx, hive, "StartupFolder", file, enabled)
        }
        Target::WinTask { name } => {
            if !arg_ok(name) {
                return Err(ApiError::invalid_params("unsupported task name"));
            }
            let flag = if enabled { "/enable" } else { "/disable" };
            let args = ["/change", "/tn", name.as_str(), flag];
            let out = ctx.runner.run("schtasks", &args)?;
            if out.success() {
                return Ok(());
            }
            let msg = format!("{} {}", out.stderr, out.stdout).to_lowercase();
            if msg.contains("denied") || msg.contains("access") {
                return ensure_ok(run_privileged(ctx, "schtasks", &args)?, "schtasks /change");
            }
            ensure_ok(out, "schtasks /change")
        }
        Target::WinService { name } => {
            if !arg_ok(name) || name.starts_with(['-', '/']) {
                return Err(ApiError::invalid_params("unsupported service name"));
            }
            let mode = if enabled { "auto" } else { "disabled" };
            let out = run_privileged(ctx, "sc.exe", &["config", name, "start=", mode])?;
            ensure_ok(out, "sc config")
        }
        Target::WinContext { hive, key, handler } => {
            if !arg_ok(handler) {
                return Err(ApiError::invalid_params("unsupported handler name"));
            }
            let base = handler.trim_start_matches('-');
            let to = if enabled {
                base.to_string()
            } else {
                format!("-{base}")
            };
            if *handler == to {
                return Ok(());
            }
            let out = powershell(ctx, *hive, &rename_handler_script(*hive, key, handler, &to))?;
            ensure_ok(out, "renaming the handler")
        }
        _ => Err(ApiError::internal("not a Windows startup item")),
    }
}

pub fn delete_run_value(ctx: &Ctx, loc: &RunLoc, name: &str) -> Result<()> {
    if !arg_ok(name) {
        return Err(ApiError::invalid_params("unsupported entry name"));
    }
    let key = format!(r"{}\{}", loc.hive.name(), loc.key);
    ensure_ok(
        reg_run(ctx, loc.hive, &["delete", &key, "/v", name, "/f"])?,
        "reg delete",
    )?;
    if let Some(sub) = loc.approved {
        // The approval record is left over otherwise. Best effort.
        let akey = format!(r"{}\{}", loc.hive.name(), approved_key(loc.hive, sub));
        let _ = reg_run(ctx, loc.hive, &["delete", &akey, "/v", name, "/f"]);
    }
    Ok(())
}

pub fn delete_task(ctx: &Ctx, name: &str) -> Result<()> {
    if !arg_ok(name) {
        return Err(ApiError::invalid_params("unsupported task name"));
    }
    let args = ["/delete", "/tn", name, "/f"];
    let out = ctx.runner.run("schtasks", &args)?;
    if out.success() {
        return Ok(());
    }
    let msg = format!("{} {}", out.stderr, out.stdout).to_lowercase();
    if msg.contains("denied") || msg.contains("access") {
        return ensure_ok(run_privileged(ctx, "schtasks", &args)?, "schtasks /delete");
    }
    ensure_ok(out, "schtasks /delete")
}

pub fn delete_context_handler(ctx: &Ctx, hive: WinHive, key: &str, handler: &str) -> Result<()> {
    if !arg_ok(handler) {
        return Err(ApiError::invalid_params("unsupported handler name"));
    }
    let full = format!(r"{}\{key}\{handler}", hive.name());
    ensure_ok(reg_run(ctx, hive, &["delete", &full, "/f"])?, "reg delete")
}

// ---------------------------------------------------------------- real registry

#[cfg(windows)]
pub struct RealStartupRegistry;

#[cfg(windows)]
mod real {
    use super::{RegData, StartupRegistry};
    use crate::features::startup::model::WinHive;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_BINARY, REG_DWORD};
    use winreg::enums::{REG_EXPAND_SZ, REG_SZ};
    use winreg::types::FromRegValue;
    use winreg::RegKey;

    fn open(hive: WinHive, key: &str) -> Option<RegKey> {
        let root = match hive {
            WinHive::Hkcu => HKEY_CURRENT_USER,
            WinHive::Hklm => HKEY_LOCAL_MACHINE,
        };
        RegKey::predef(root)
            .open_subkey_with_flags(key, KEY_READ)
            .ok()
    }

    impl StartupRegistry for super::RealStartupRegistry {
        fn values(&self, hive: WinHive, key: &str) -> Vec<(String, RegData)> {
            let Some(k) = open(hive, key) else {
                return Vec::new();
            };
            k.enum_values()
                .filter_map(|r| r.ok())
                .filter_map(|(name, raw)| {
                    let d = match raw.vtype {
                        REG_SZ | REG_EXPAND_SZ => RegData::Str(String::from_reg_value(&raw).ok()?),
                        REG_DWORD => RegData::Dword(u32::from_reg_value(&raw).ok()?),
                        REG_BINARY => RegData::Binary(raw.bytes.to_vec()),
                        _ => return None,
                    };
                    Some((name, d))
                })
                .collect()
        }
        fn subkeys(&self, hive: WinHive, key: &str) -> Vec<String> {
            open(hive, key)
                .map(|k| k.enum_keys().filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        }
        fn default_value(&self, hive: WinHive, key: &str) -> Option<String> {
            open(hive, key)?.get_value::<String, _>("").ok()
        }
        fn key_exists(&self, hive: WinHive, key: &str) -> bool {
            open(hive, key).is_some()
        }
    }
}
