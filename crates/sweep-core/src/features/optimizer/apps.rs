//! Grouping startup items and running processes into "apps".
//!
//! An app is a user-facing program, identified by a normalised executable name (or, on
//! macOS, the outermost `.app` bundle). It gets:
//! - the user-level startup items that launch it (`startupIds`, systemd user units as
//!   `serviceIds`),
//! - its running processes (matched strictly, see [`impact::proc_matches`]).
//!
//! Never part of an app: critical startup items, the desktop session (compositor, shell,
//! audio server, `explorer.exe`, `Finder`, `Dock`, `WindowServer`...), terminals, ClearSweep
//! itself and any other user's processes.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::ctx::{Ctx, Os};
use crate::features::startup::impact::{base_key, exe_from_command, proc_matches};
use crate::features::startup::linux::{parse_desktop, strip_field_codes};
use crate::features::startup::model::{Entry, Kind};
use crate::features::startup::needs_admin;
use crate::procs::ProcDetail;

/// Process names that belong to the desktop session or would take ClearSweep (or the
/// terminal it was started from) down with them. Compared lower-case, exact or as a prefix
/// where the entry ends in `*`.
const SESSION_PROCESSES: &[&str] = &[
    "systemd",
    "systemd-*",
    "dbus-daemon",
    "dbus-broker*",
    "gnome-shell",
    "gnome-session*",
    "gnome-keyring*",
    "gnome-terminal*",
    "gnome-software",
    "mutter",
    "plasmashell",
    "kwin*",
    "kded*",
    "kglobalaccel*",
    "ksmserver",
    "xorg",
    "xwayland",
    "xfce4-*",
    "xfwm4",
    "sway",
    "wayfire",
    "hyprland",
    "i3",
    "openbox",
    "gdm*",
    "lightdm",
    "sddm*",
    "pipewire*",
    "wireplumber",
    "pulseaudio",
    "polkitd",
    "polkit-*",
    "ibus-*",
    "fcitx*",
    "at-spi*",
    "gvfs*",
    "konsole",
    "xterm",
    "alacritty",
    "kitty",
    "wezterm*",
    "terminator",
    "tilix",
    "lxterminal",
    "foot",
    "urxvt*",
    "tmux*",
    "screen",
    "sshd*",
    "explorer.exe",
    "dwm.exe",
    "winlogon.exe",
    "csrss.exe",
    "services.exe",
    "svchost.exe",
    "taskhostw.exe",
    "sihost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "searchhost.exe",
    "runtimebroker.exe",
    "ctfmon.exe",
    "conhost.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "windowsterminal.exe",
    "wt.exe",
    "finder",
    "dock",
    "windowserver",
    "loginwindow",
    "systemuiserver",
    "controlcenter",
    "notificationcenter",
    "launchd",
    "terminal",
    "iterm2",
    "clearsweep*",
    "sweep-*",
];

fn glob_ci(pat: &str, name: &str) -> bool {
    let n = name.to_lowercase();
    match pat.strip_suffix('*') {
        Some(prefix) => n.starts_with(prefix),
        None => n == pat,
    }
}

pub fn is_session_process(p: &ProcDetail) -> bool {
    SESSION_PROCESSES.iter().any(|pat| glob_ci(pat, &p.name))
        || p.exe.as_deref().is_some_and(|e| {
            let k = base_key(e);
            SESSION_PROCESSES.iter().any(|pat| glob_ci(pat, &k))
        })
}

/// Security software is never put to sleep.
const SECURITY_EXACT: &[&str] = &["avg", "avp", "ns", "ekrn", "wrsa", "vsserv", "savservice"];
const SECURITY_FRAGMENTS: &[&str] = &[
    "defender",
    "msmpeng",
    "securityhealth",
    "windefend",
    "smartscreen",
    "malwarebytes",
    "mbam",
    "clamav",
    "clamd",
    "freshclam",
    "avast",
    "kaspersky",
    "norton",
    "mcafee",
    "mfemms",
    "bitdefender",
    "bdservicehost",
    "eset",
    "sophos",
    "crowdstrike",
    "falcon-sensor",
    "csfalcon",
    "sentinelone",
    "sentinelagent",
    "webroot",
    "avira",
    "avguard",
    "trendmicro",
    "trend-micro",
    "tmbmsrv",
    "fail2ban",
    "auditd",
    "apparmor",
    "little-snitch",
    "littlesnitch",
    "carbonblack",
    "cylance",
    "f-secure",
    "comodo",
    "zonealarm",
    "firewall",
    "antivirus",
    "endpoint",
    "yubikey",
    "keepass",
    "bitwarden",
    "1password",
    "gpg-agent",
];

pub fn is_security_software(key_or_name: &str) -> bool {
    let k = key_or_name.to_lowercase();
    SECURITY_EXACT.contains(&k.as_str()) || SECURITY_FRAGMENTS.iter().any(|f| k.contains(f))
}

/// `.../Foo.app/Contents/...` -> `Foo`, the outermost bundle.
pub fn bundle_name(exe: &str) -> Option<String> {
    let norm = exe.replace('\\', "/");
    let i = norm.find(".app/")?;
    let head = &norm[..i];
    let name = head.rsplit('/').next()?;
    (!name.is_empty()).then(|| name.to_string())
}

const GENERIC_LAUNCHERS: &[&str] = &[
    "flatpak", "snap", "env", "sh", "bash", "dash", "zsh", "xdg-open", "python", "python3",
    "java", "node", "electron", "wine", "gtk-launch", "dbus-launch", "dbus-send", "open",
    "cmd", "cmd.exe", "powershell", "powershell.exe", "wscript.exe", "rundll32.exe",
];

/// The real target of a `flatpak run app.id`, `snap run app`, `gtk-launch app` command.
fn launcher_target(command: &str) -> Option<String> {
    let toks = crate::features::startup::impact::split_words(command);
    let first = toks.first()?;
    let base = base_key(first);
    match base.as_str() {
        "flatpak" | "snap" => {
            let mut it = toks.iter().skip(1);
            it.find(|t| *t == "run")?;
            it.find(|t| !t.starts_with('-')).cloned()
        }
        "gtk-launch" => toks.get(1).cloned(),
        _ => None,
    }
}

/// App key of a startup entry.
pub fn entry_app_key(e: &Entry) -> String {
    if let Some(exe) = &e.exe {
        if let Some(b) = bundle_name(exe) {
            return base_key(&b);
        }
    }
    if let Some(t) = launcher_target(&e.item.command) {
        return base_key(&t);
    }
    if let Some(exe) = &e.exe {
        let k = base_key(exe);
        if !k.is_empty() && !GENERIC_LAUNCHERS.contains(&k.as_str()) {
            return k;
        }
    }
    base_key(&e.item.name)
}

/// Can the optimizer manage this item? User-level, not critical, switchable, no admin needed.
pub fn eligible(e: &Entry) -> bool {
    !e.item.critical
        && e.item.can_disable
        && !needs_admin(&e.target)
        && matches!(
            e.item.kind,
            Kind::Autostart | Kind::LaunchAgent | Kind::Service
        )
}

/// What `entry_app_key` groups: eligible and not a service that is not a user unit.
pub fn is_app_item(e: &Entry) -> bool {
    eligible(e)
}

#[derive(Debug, Clone)]
pub struct App {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub procs: Vec<ProcDetail>,
    pub startup_ids: Vec<String>,
    pub service_ids: Vec<String>,
    pub protected: bool,
}

struct Builder {
    name: String,
    icon: Option<String>,
    hints: Vec<String>,
    procs: Vec<ProcDetail>,
    startup_ids: Vec<String>,
    service_ids: Vec<String>,
}

impl Builder {
    fn new(name: &str) -> Self {
        Builder {
            name: name.to_string(),
            icon: None,
            hints: Vec::new(),
            procs: Vec::new(),
            startup_ids: Vec::new(),
            service_ids: Vec::new(),
        }
    }
}

struct CatalogApp {
    name: String,
    icon: Option<String>,
    hint: String,
}

/// Desktop entries of installed applications (Linux). Only the executable, name and icon.
fn linux_catalog(ctx: &Ctx) -> Vec<CatalogApp> {
    let dirs = [
        ctx.env.sys_path("/usr/share/applications"),
        ctx.env.sys_path("/usr/local/share/applications"),
        ctx.env.user_data_dir.join("applications"),
        ctx.env.sys_path("/var/lib/flatpak/exports/share/applications"),
        ctx.env
            .user_data_dir
            .join("flatpak/exports/share/applications"),
        ctx.env.sys_path("/var/lib/snapd/desktop/applications"),
    ];
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "desktop") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&p) else {
                continue;
            };
            if text.lines().any(|l| l.trim() == "NoDisplay=true") {
                continue;
            }
            let dsk = parse_desktop(&text);
            let (Some(name), Some(exec)) = (dsk.name, dsk.exec) else {
                continue;
            };
            let Some(hint) = exe_from_command(&strip_field_codes(&exec)) else {
                continue;
            };
            if GENERIC_LAUNCHERS.contains(&base_key(&hint).as_str()) {
                continue;
            }
            out.push(CatalogApp {
                name,
                icon: dsk.icon,
                hint,
            });
        }
    }
    out
}

fn windows_user_app(exe: &str) -> bool {
    let l = exe.to_lowercase().replace('/', "\\");
    (l.contains("\\program files") || l.contains("\\appdata\\") || l.contains("\\programdata\\"))
        && !l.contains("\\windows\\")
        && !l.contains("\\windowsapps\\microsoft.")
}

fn mac_user_app(exe: &str) -> bool {
    exe.contains(".app/") && !exe.starts_with("/System/") && !exe.starts_with("/usr/")
}

fn key_of_proc(os: Os, p: &ProcDetail) -> String {
    if os == Os::MacOs {
        if let Some(b) = p.exe.as_deref().and_then(bundle_name) {
            return base_key(&b);
        }
    }
    base_key(p.exe.as_deref().unwrap_or(&p.name))
}

/// Group `entries` and `procs` into apps. `own_pid` is excluded.
pub fn build_apps(ctx: &Ctx, entries: &[Entry], procs: &[ProcDetail], own_pid: u32) -> Vec<App> {
    let os = ctx.env.os;
    let mut builders: BTreeMap<String, Builder> = BTreeMap::new();
    let mut protected_keys: std::collections::HashSet<String> = Default::default();

    for e in entries {
        let key = entry_app_key(e);
        if key.is_empty() {
            continue;
        }
        if e.item.critical {
            protected_keys.insert(key);
            continue;
        }
        if !is_app_item(e) {
            continue;
        }
        let b = builders
            .entry(key.clone())
            .or_insert_with(|| Builder::new(&display_name(e)));
        if b.icon.is_none() {
            b.icon = e.icon.clone();
        }
        if let Some(h) = &e.exe {
            if !b.hints.contains(h) {
                b.hints.push(h.clone());
            }
        }
        if e.item.kind == Kind::Service {
            b.service_ids.push(e.item.id.clone());
        } else {
            b.startup_ids.push(e.item.id.clone());
        }
    }

    let catalog = if os == Os::Linux {
        linux_catalog(ctx)
    } else {
        Vec::new()
    };

    let candidates: Vec<&ProcDetail> = procs
        .iter()
        .filter(|p| p.is_mine && p.pid != own_pid && p.pid > 1 && !is_session_process(p))
        .collect();

    for p in candidates {
        let pkey = key_of_proc(os, p);
        // 1. an app that already exists
        let target = if builders.contains_key(&pkey) {
            Some(pkey.clone())
        } else {
            builders
                .iter()
                .find(|(_, b)| b.hints.iter().any(|h| proc_matches(h, p)))
                .map(|(k, _)| k.clone())
        };
        if let Some(k) = target {
            if let Some(b) = builders.get_mut(&k) {
                b.procs.push(p.clone());
            }
            continue;
        }
        // 2. a program with a desktop entry / app bundle / installed program
        let created = match os {
            Os::Linux => catalog.iter().find(|c| proc_matches(&c.hint, p)).map(|c| {
                let mut b = Builder::new(&c.name);
                b.icon = c.icon.clone();
                b.hints.push(c.hint.clone());
                (base_key(&c.hint), b)
            }),
            Os::MacOs => p
                .exe
                .as_deref()
                .filter(|e| mac_user_app(e))
                .and_then(bundle_name)
                .map(|n| (base_key(&n), Builder::new(&n))),
            Os::Windows => p
                .exe
                .as_deref()
                .filter(|e| windows_user_app(e))
                .map(|e| (base_key(e), Builder::new(&pretty(&base_key(e))))),
        };
        if let Some((k, mut b)) = created {
            if let Some(existing) = builders.get_mut(&k) {
                existing.procs.push(p.clone());
            } else {
                b.procs.push(p.clone());
                builders.insert(k, b);
            }
        }
    }

    builders
        .into_iter()
        .map(|(id, b)| {
            let protected = protected_keys.contains(&id)
                || is_security_software(&id)
                || is_security_software(&b.name)
                || b.procs.iter().any(|p| is_security_software(&p.name));
            App {
                id,
                name: b.name,
                icon: b.icon,
                procs: b.procs,
                startup_ids: b.startup_ids,
                service_ids: b.service_ids,
                protected,
            }
        })
        .collect()
}

fn display_name(e: &Entry) -> String {
    let n = e.item.name.trim();
    if n.is_empty() {
        pretty(&entry_app_key(e))
    } else {
        n.to_string()
    }
}

fn pretty(key: &str) -> String {
    let mut c = key.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Convenience for tests / callers: does `dir` look like an existing directory?
#[allow(dead_code)]
pub fn is_dir(p: &Path) -> bool {
    p.is_dir()
}
