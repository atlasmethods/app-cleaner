//! Linux startup items: XDG autostart entries, systemd services and `@reboot` cron lines.
//!
//! - **XDG autostart** (`~/.config/autostart/*.desktop`, `/etc/xdg/autostart/*.desktop`).
//!   A user file replaces the system file of the same name. Disabling a user entry sets
//!   `Hidden=true` in place (every other byte of the file is untouched); disabling a system
//!   entry writes a user override that is a copy of the system file plus `Hidden=true` (the
//!   XDG way, no root needed). Enabling removes the `Hidden` line again and deletes an
//!   override that has become a plain copy of the system file.
//! - **systemd** units through `systemctl` (`--user` needs no privileges; system units are
//!   changed elevated).
//! - **cron**: `@reboot` lines from `crontab -l`. Disabling prefixes the line with
//!   [`CRON_PREFIX`]; every other line is written back exactly as it was.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ctx::Ctx;
use crate::elevate::run_privileged;
use crate::error::{ApiError, Result};
use crate::fsutil::{atomic_write, random_id};
use crate::pkgutil::summarize;
use crate::safety::{ExcludeSet, SafeDeleter};

use super::impact::exe_from_command;
use super::model::{Entry, Kind, Scope, StartupItem, Target};

pub const CRON_PREFIX: &str = "# clearsweep-disabled: ";

// ---------------------------------------------------------------- .desktop files

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Desktop {
    pub name: Option<String>,
    pub exec: Option<String>,
    pub icon: Option<String>,
    pub hidden: bool,
    /// `X-GNOME-Autostart-enabled`
    pub gnome_enabled: Option<bool>,
}

impl Desktop {
    /// Autostart state: `OnlyShowIn` / `NotShowIn` are deliberately ignored.
    pub fn enabled(&self) -> bool {
        !self.hidden && self.gnome_enabled != Some(false)
    }
}

fn is_group_header(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('[') && t.ends_with(']')
}

fn key_of(line: &str) -> Option<(&str, &str)> {
    let t = line.trim_start();
    if t.starts_with('#') {
        return None;
    }
    let (k, v) = t.split_once('=')?;
    Some((k.trim(), v.trim()))
}

fn truthy(v: &str) -> bool {
    v.eq_ignore_ascii_case("true") || v == "1"
}

pub fn parse_desktop(text: &str) -> Desktop {
    let mut d = Desktop::default();
    let mut in_entry = false;
    for line in text.lines() {
        if is_group_header(line) {
            in_entry = line.trim() == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((k, v)) = key_of(line) else { continue };
        match k {
            "Name" => d.name = Some(v.to_string()),
            "Exec" => d.exec = Some(v.to_string()),
            "Icon" => d.icon = Some(v.to_string()),
            "Hidden" => d.hidden = truthy(v),
            "X-GNOME-Autostart-enabled" => d.gnome_enabled = Some(truthy(v)),
            _ => {}
        }
    }
    d
}

/// Range of line indices belonging to the `[Desktop Entry]` group (header excluded).
fn entry_group(lines: &[&str]) -> Option<(usize, usize)> {
    let start = lines.iter().position(|l| l.trim() == "[Desktop Entry]")? + 1;
    let end = lines[start..]
        .iter()
        .position(|l| is_group_header(l))
        .map_or(lines.len(), |i| start + i);
    Some((start, end))
}

fn eol_of(line: &str) -> &'static str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// `text` with `Hidden=true` set in the `[Desktop Entry]` group. Everything else is kept
/// byte for byte.
pub fn set_hidden(text: &str) -> Result<String> {
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let (start, end) = entry_group(&refs)
        .ok_or_else(|| ApiError::invalid_params("not a valid .desktop file: no [Desktop Entry]"))?;
    let mut found = false;
    for l in &mut lines[start..end] {
        if matches!(key_of(l), Some(("Hidden", _))) {
            let eol = if l.ends_with("\r\n") {
                "\r\n"
            } else if l.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            *l = format!("Hidden=true{eol}");
            found = true;
        }
    }
    if found {
        return Ok(lines.concat());
    }
    // Insert after the last non-blank line of the group.
    let mut at = start;
    for (i, l) in lines[start..end].iter().enumerate() {
        if !l.trim().is_empty() {
            at = start + i + 1;
        }
    }
    let eol = if at > 0 { eol_of(&lines[at - 1]) } else { "\n" };
    let last_unterminated = at > 0 && !lines[at - 1].ends_with('\n');
    if last_unterminated {
        // The group ends the file without a newline: give that line one and leave the new
        // last line unterminated, so removing it again restores the original bytes.
        lines[at - 1].push_str(eol);
        lines.insert(at, "Hidden=true".to_string());
    } else {
        lines.insert(at, format!("Hidden=true{eol}"));
    }
    Ok(lines.concat())
}

/// `text` without `Hidden=` and `X-GNOME-Autostart-enabled=false` lines in the group.
pub fn clear_hidden(text: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let Some((start, end)) = entry_group(&lines) else {
        return text.to_string();
    };
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for (i, l) in lines.iter().enumerate() {
        let drop = i >= start
            && i < end
            && match key_of(l) {
                Some(("Hidden", _)) => true,
                Some(("X-GNOME-Autostart-enabled", v)) => !truthy(v),
                _ => false,
            };
        if drop {
            // Removing an unterminated final line: also drop the newline we added before it.
            if !l.ends_with('\n') && i + 1 == lines.len() {
                if let Some(prev) = out.last_mut() {
                    if prev.ends_with("\r\n") {
                        prev.truncate(prev.len() - 2);
                    } else if prev.ends_with('\n') {
                        prev.truncate(prev.len() - 1);
                    }
                }
            }
            continue;
        }
        out.push((*l).to_string());
    }
    out.concat()
}

fn has_exec(text: &str) -> bool {
    parse_desktop(text).exec.is_some()
}

// ---------------------------------------------------------------- XDG collection

pub fn user_autostart_dir(ctx: &Ctx) -> PathBuf {
    ctx.env.config_dir.join("autostart")
}

pub fn system_autostart_dir(ctx: &Ctx) -> PathBuf {
    ctx.env.sys_path("/etc/xdg/autostart")
}

fn desktop_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            (name.ends_with(".desktop") && p.is_file()).then_some((name, p))
        })
        .collect();
    v.sort();
    v
}

/// Autostart entries never to be touched: the desktop session (keyring, polkit agent,
/// settings daemon, audio, accessibility bus...) depends on them.
const CRITICAL_XDG_PREFIXES: &[&str] = &[
    "polkit",
    "gnome-keyring",
    "org.gnome.settingsdaemon",
    "org.gnome.settings-daemon",
    "gnome-settings-daemon",
    "gsettings-data-convert",
    "gnome-initial-setup",
    "pulseaudio",
    "pipewire",
    "wireplumber",
    "at-spi",
    "xdg-user-dirs",
    "kded",
    "kwalletd",
    "org.kde.kwalletd",
    "org.kde.plasma",
    "xfce4-settings",
    "xfsettingsd",
    "lxqt-session",
    "im-launch",
    "ibus",
    "gnome-shell",
    "plasma-",
    "nm-applet",
];

pub fn xdg_is_critical(file: &str) -> bool {
    let f = file.to_lowercase();
    CRITICAL_XDG_PREFIXES.iter().any(|p| f.starts_with(p))
}

pub fn collect_xdg(ctx: &Ctx) -> Vec<Entry> {
    let user_dir = user_autostart_dir(ctx);
    let sys_dir = system_autostart_dir(ctx);
    let user: HashMap<String, PathBuf> = desktop_files(&user_dir).into_iter().collect();
    let sys: HashMap<String, PathBuf> = desktop_files(&sys_dir).into_iter().collect();
    let mut names: Vec<&String> = user.keys().chain(sys.keys()).collect();
    names.sort();
    names.dedup();
    let mut out = Vec::new();
    for file in names {
        let u = user
            .get(file)
            .and_then(|p| fs::read_to_string(p).ok())
            .map(|t| parse_desktop(&t));
        let s = sys
            .get(file)
            .and_then(|p| fs::read_to_string(p).ok())
            .map(|t| parse_desktop(&t));
        if u.is_none() && s.is_none() {
            continue;
        }
        let exec = u
            .as_ref()
            .and_then(|d| d.exec.clone())
            .or_else(|| s.as_ref().and_then(|d| d.exec.clone()));
        let Some(exec) = exec else { continue };
        let name = u
            .as_ref()
            .and_then(|d| d.name.clone())
            .or_else(|| s.as_ref().and_then(|d| d.name.clone()))
            .unwrap_or_else(|| file.trim_end_matches(".desktop").to_string());
        let icon = u
            .as_ref()
            .and_then(|d| d.icon.clone())
            .or_else(|| s.as_ref().and_then(|d| d.icon.clone()));
        let enabled = match (&u, &s) {
            (Some(d), _) => d.enabled(),
            (None, Some(d)) => d.enabled(),
            _ => true,
        };
        let system_scope = s.is_some();
        let critical = xdg_is_critical(file);
        let location = if u.is_some() {
            user.get(file).cloned()
        } else {
            sys.get(file).cloned()
        }
        .unwrap_or_default();
        let scope = if system_scope {
            Scope::System
        } else {
            Scope::User
        };
        let mut item = StartupItem::new(
            format!(
                "xdg:{}:{file}",
                if system_scope { "system" } else { "user" }
            ),
            name,
            Kind::Autostart,
            scope,
        );
        item.command = exec.clone();
        item.location = location.to_string_lossy().into_owned();
        item.enabled = enabled;
        item.critical = critical;
        item.can_disable = !critical;
        item.can_delete = !system_scope && !critical;
        let mut e = Entry::new(
            item,
            Target::Xdg {
                file: file.clone(),
                user_path: user_dir.join(file),
                system_path: sys.get(file).cloned(),
            },
        );
        e.exe = exe_from_command(&strip_field_codes(&exec));
        e.icon = icon;
        out.push(e);
    }
    out
}

/// Remove `%f %U ...` field codes from an `Exec=` value.
pub fn strip_field_codes(exec: &str) -> String {
    exec.split_whitespace()
        .filter(|t| !(t.len() == 2 && t.starts_with('%')))
        .collect::<Vec<_>>()
        .join(" ")
}

fn write_text(path: &Path, text: &str) -> Result<()> {
    atomic_write(path, text.as_bytes())
        .map_err(|e| ApiError::io(format!("could not write {}: {e}", path.display())))
}

/// Delete `path` (a file inside `dir`) with the shared safety checks.
pub fn safe_remove_file(ctx: &Ctx, dir: &Path, path: &Path) -> Result<()> {
    let d = SafeDeleter::new(&ctx.env, dir, ExcludeSet::empty())?;
    d.remove_file(path).map_err(|e| e.to_api(path))?;
    Ok(())
}

pub fn xdg_set_enabled(
    ctx: &Ctx,
    user_path: &Path,
    system_path: Option<&Path>,
    enabled: bool,
) -> Result<()> {
    let user_text = fs::read_to_string(user_path).ok();
    let sys_text = system_path.and_then(|p| fs::read_to_string(p).ok());
    if !enabled {
        let (src, _) = match (&user_text, &sys_text) {
            (Some(t), _) => (t.clone(), true),
            (None, Some(t)) => (t.clone(), false),
            (None, None) => {
                return Err(ApiError::not_found("the autostart entry no longer exists"))
            }
        };
        return write_text(user_path, &set_hidden(&src)?);
    }
    match (user_text, sys_text) {
        (Some(t), sys) => {
            let cleaned = clear_hidden(&t);
            let plain_override = sys.as_ref().is_some_and(|s| {
                *s == cleaned || (!has_exec(&cleaned) && parse_desktop(s).enabled())
            });
            if plain_override {
                let dir = user_path
                    .parent()
                    .ok_or_else(|| ApiError::internal("autostart path has no parent"))?;
                safe_remove_file(ctx, dir, user_path)
            } else {
                write_text(user_path, &cleaned)
            }
        }
        (None, Some(s)) => {
            // The system file itself is hidden: a user file that is not.
            if parse_desktop(&s).enabled() {
                Ok(())
            } else {
                write_text(user_path, &clear_hidden(&s))
            }
        }
        (None, None) => Err(ApiError::not_found("the autostart entry no longer exists")),
    }
}

// ---------------------------------------------------------------- systemd

/// `systemctl list-unit-files` rows: `(unit, state)`. The optional third (preset) column is
/// ignored, and rows that do not look like `name.service state` are skipped.
pub fn parse_unit_files(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let unit = it.next()?;
            let state = it.next()?;
            unit.ends_with(".service")
                .then(|| (unit.to_string(), state.to_string()))
        })
        .collect()
}

/// Services that must keep working for the machine to boot, log in, get a network or keep
/// its security tooling. A prefix match (case-insensitive) on the unit name. Documented
/// here because it is a safety policy: the UI shows these greyed out and locked.
///
/// - init / login: `systemd-*`, `dbus*`, `getty*`, `serial-getty*`, `console-*`, `user@*`,
///   `user-runtime-dir@*`, `plymouth*`, `udev*`, `polkit*`, `accounts-daemon`, `rtkit-daemon`,
///   `upower`, `udisks2`, `logrotate`
/// - display managers: `gdm*`, `sddm`, `lightdm`, `lxdm`, `greetd`, `display-manager`
/// - network: `networkmanager*`, `network*`, `wpa_supplicant*`, `iwd`, `dhcpcd`, `dhclient`,
///   `resolvconf`, `connman`, `netplan*`, `ifup*`
/// - security: `apparmor`, `ufw`, `firewalld`, `nftables`, `iptables`, `netfilter-persistent`,
///   `auditd`, `selinux*`, `fail2ban`, `clamav*`, `apport`
/// - logging / scheduling: `rsyslog`, `syslog*`, `cron`, `crond`, `anacron`, `atd`
/// - storage: `lvm2*`, `dm-event`, `multipathd`, `mdmonitor`, `fstrim`, `swap*`, `zfs*`
///
/// Deliberately NOT critical (the user may reasonably choose): `ssh`/`sshd` (warned),
/// `bluetooth`, `cups*`, `avahi-daemon`, `docker`, `snapd`, `ModemManager`, `smbd`...
pub const CRITICAL_UNIT_PREFIXES: &[&str] = &[
    "systemd-",
    "dbus",
    "getty",
    "serial-getty",
    "console-",
    "user@",
    "user-runtime-dir@",
    "plymouth",
    "udev",
    "polkit",
    "accounts-daemon",
    "rtkit-daemon",
    "upower",
    "udisks2",
    "logrotate",
    "gdm",
    "sddm",
    "lightdm",
    "lxdm",
    "greetd",
    "display-manager",
    "networkmanager",
    "network",
    "wpa_supplicant",
    "iwd",
    "dhcpcd",
    "dhclient",
    "resolvconf",
    "connman",
    "netplan",
    "ifup",
    "apparmor",
    "ufw",
    "firewalld",
    "nftables",
    "iptables",
    "netfilter-persistent",
    "auditd",
    "selinux",
    "fail2ban",
    "clamav",
    "apport",
    "rsyslog",
    "syslog",
    "cron",
    "crond",
    "anacron",
    "atd.",
    "lvm2",
    "dm-event",
    "multipathd",
    "mdmonitor",
    "fstrim",
    "swap",
    "zfs",
];

/// User-session units that the desktop needs.
const CRITICAL_USER_UNIT_PREFIXES: &[&str] = &[
    "dbus",
    "pipewire",
    "pulseaudio",
    "wireplumber",
    "gnome-",
    "org.gnome.",
    "plasma-",
    "xdg-",
    "at-spi",
    "gvfs",
    "gpg-agent",
    "ssh-agent",
    "gcr-",
    "p11-kit",
    "graphical-session",
    "systemd-",
    "app-",
    "ibus",
    "evolution-",
];

pub fn unit_is_critical(unit: &str, user: bool) -> bool {
    let u = unit.to_lowercase();
    if u.ends_with("@.service") {
        return true;
    }
    if CRITICAL_UNIT_PREFIXES.iter().any(|p| u.starts_with(p)) {
        return true;
    }
    user && CRITICAL_USER_UNIT_PREFIXES.iter().any(|p| u.starts_with(p))
}

pub fn unit_warning(unit: &str) -> Option<String> {
    let u = unit.to_lowercase();
    if u.starts_with("ssh.") || u.starts_with("sshd.") {
        Some("Disabling SSH stops remote logins to this machine after the next restart.".into())
    } else if u.starts_with("docker.") || u.starts_with("containerd.") {
        Some("Containers will not start automatically after a restart.".into())
    } else if u.starts_with("snapd.") {
        Some("Snap applications may stop working.".into())
    } else if u.starts_with("bluetooth.") {
        Some("Bluetooth devices will not connect until it is enabled again.".into())
    } else {
        None
    }
}

fn unit_dirs(ctx: &Ctx, user: bool) -> Vec<PathBuf> {
    if user {
        vec![
            ctx.env.config_dir.join("systemd/user"),
            ctx.env.sys_path("/etc/systemd/user"),
            ctx.env.sys_path("/usr/lib/systemd/user"),
            ctx.env.sys_path("/lib/systemd/user"),
        ]
    } else {
        vec![
            ctx.env.sys_path("/etc/systemd/system"),
            ctx.env.sys_path("/usr/lib/systemd/system"),
            ctx.env.sys_path("/lib/systemd/system"),
        ]
    }
}

/// `ExecStart=` of a unit file, without systemd's `-@+!:` prefixes.
pub fn unit_exec_start(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let v = l.trim().strip_prefix("ExecStart=")?;
        let v = v.trim_start_matches(['-', '@', '+', '!', ':']).trim();
        (!v.is_empty()).then(|| v.to_string())
    })
}

fn unit_info(ctx: &Ctx, unit: &str, user: bool) -> (Option<String>, String) {
    for d in unit_dirs(ctx, user) {
        let p = d.join(unit);
        if let Ok(t) = fs::read_to_string(&p) {
            return (unit_exec_start(&t), p.to_string_lossy().into_owned());
        }
    }
    (
        None,
        if user {
            "systemd user unit".into()
        } else {
            "systemd unit".into()
        },
    )
}

pub const SYSTEMCTL_USER_LIST: &[&str] = &[
    "--user",
    "list-unit-files",
    "--type=service",
    "--no-legend",
    "--no-pager",
];
pub const SYSTEMCTL_SYSTEM_LIST: &[&str] = &[
    "list-unit-files",
    "--type=service",
    "--state=enabled,disabled",
    "--no-legend",
    "--no-pager",
];

pub fn collect_systemd(ctx: &Ctx) -> Vec<Entry> {
    let mut out = Vec::new();
    if ctx.runner.which("systemctl").is_none() {
        return out;
    }
    for (user, args) in [(true, SYSTEMCTL_USER_LIST), (false, SYSTEMCTL_SYSTEM_LIST)] {
        let Ok(o) = ctx.runner.run("systemctl", args) else {
            continue;
        };
        // A non-zero exit can still carry usable output (systemd warns about odd units).
        for (unit, state) in parse_unit_files(&o.stdout) {
            let enabled = match state.as_str() {
                "enabled" | "enabled-runtime" => true,
                "disabled" => false,
                _ => continue,
            };
            if unit.ends_with("@.service") {
                continue;
            }
            let critical = unit_is_critical(&unit, user);
            let stem = unit.trim_end_matches(".service").to_string();
            let (exec, location) = unit_info(ctx, &unit, user);
            let mut item = StartupItem::new(
                format!("systemd:{}:{unit}", if user { "user" } else { "system" }),
                stem.clone(),
                Kind::Service,
                if user { Scope::User } else { Scope::System },
            );
            item.command = exec.clone().unwrap_or_else(|| unit.clone());
            item.location = location;
            item.enabled = enabled;
            item.critical = critical;
            item.can_disable = !critical;
            item.warning = unit_warning(&unit);
            let mut e = Entry::new(
                item,
                Target::Systemd {
                    unit: unit.clone(),
                    user,
                },
            );
            e.exe = exec.as_deref().and_then(exe_from_command).or(Some(stem));
            out.push(e);
        }
    }
    out
}

pub fn systemd_set_enabled(ctx: &Ctx, unit: &str, user: bool, enabled: bool) -> Result<()> {
    if !valid_unit_name(unit) {
        return Err(ApiError::invalid_params("unsafe unit name"));
    }
    let verb = if enabled { "enable" } else { "disable" };
    let out = if user {
        ctx.runner.run("systemctl", &["--user", verb, unit])?
    } else {
        run_privileged(ctx, "systemctl", &[verb, unit])?
    };
    if out.success() {
        Ok(())
    } else {
        Err(ApiError::io(format!(
            "systemctl {verb} {unit} failed: {}",
            summarize(&out)
        )))
    }
}

fn valid_unit_name(u: &str) -> bool {
    !u.is_empty()
        && !u.starts_with('-')
        && u.ends_with(".service")
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | ':' | '\\'))
}

// ---------------------------------------------------------------- cron

/// Stable, short hash of a line (FNV-1a), for cron item ids.
pub fn line_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", (h ^ (h >> 32)) as u32)
}

/// `Some((is_enabled, underlying_line))` when `line` is an `@reboot` entry or a disabled one.
pub fn cron_reboot_line(line: &str) -> Option<(bool, String)> {
    let l = line.trim_end_matches(['\r', '\n']);
    let (enabled, body) = match l.strip_prefix(CRON_PREFIX) {
        Some(rest) => (false, rest),
        None => (true, l),
    };
    let t = body.trim_start();
    let rest = t.strip_prefix("@reboot")?;
    (rest.starts_with(char::is_whitespace) && !rest.trim().is_empty())
        .then(|| (enabled, body.to_string()))
}

pub fn read_crontab(ctx: &Ctx) -> Option<String> {
    ctx.runner.which("crontab")?;
    let o = ctx.runner.run("crontab", &["-l"]).ok()?;
    // Exit 1 with "no crontab for user" is the normal empty case.
    o.success().then_some(o.stdout)
}

pub fn collect_cron(ctx: &Ctx) -> Vec<Entry> {
    let Some(text) = read_crontab(ctx) else {
        return Vec::new();
    };
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((enabled, body)) = cron_reboot_line(line) else {
            continue;
        };
        let nth = {
            let n = seen.entry(body.clone()).or_insert(0);
            let cur = *n;
            *n += 1;
            cur
        };
        let cmd = body
            .trim_start()
            .trim_start_matches("@reboot")
            .trim()
            .to_string();
        let mut item = StartupItem::new(
            format!("cron:{}:{nth}", line_hash(&body)),
            cmd.split_whitespace()
                .next()
                .map(|c| c.rsplit('/').next().unwrap_or(c).to_string())
                .unwrap_or_else(|| "@reboot".into()),
            Kind::Cron,
            Scope::User,
        );
        item.command = cmd.clone();
        item.location = "crontab (@reboot)".into();
        item.enabled = enabled;
        item.can_delete = true;
        let mut e = Entry::new(item, Target::Cron { line: body, nth });
        e.exe = exe_from_command(&cmd);
        out.push(e);
    }
    out
}

/// The crontab text with the `nth` occurrence of `line` switched. Every other line, and the
/// line endings, are returned exactly as they were.
pub fn cron_toggle(text: &str, line: &str, nth: usize, enabled: bool) -> Result<String> {
    let mut count = 0usize;
    let mut done = false;
    let mut out = String::with_capacity(text.len() + CRON_PREFIX.len());
    for raw in text.split_inclusive('\n') {
        let body = raw.trim_end_matches(['\r', '\n']);
        let term = &raw[body.len()..];
        if let Some((is_enabled, underlying)) = cron_reboot_line(raw) {
            if underlying == line {
                if count == nth {
                    done = true;
                    if enabled && !is_enabled {
                        out.push_str(&underlying);
                        out.push_str(term);
                    } else if !enabled && is_enabled {
                        out.push_str(CRON_PREFIX);
                        out.push_str(body);
                        out.push_str(term);
                    } else {
                        out.push_str(raw);
                    }
                    count += 1;
                    continue;
                }
                count += 1;
            }
        }
        out.push_str(raw);
    }
    if !done {
        return Err(ApiError::not_found("the cron entry no longer exists"));
    }
    Ok(out)
}

/// The crontab text without the `nth` occurrence of `line` (enabled or disabled form).
pub fn cron_remove(text: &str, line: &str, nth: usize) -> Result<String> {
    let mut count = 0usize;
    let mut done = false;
    let mut out = String::with_capacity(text.len());
    for raw in text.split_inclusive('\n') {
        if let Some((_, underlying)) = cron_reboot_line(raw) {
            if underlying == line {
                if count == nth {
                    done = true;
                    count += 1;
                    continue;
                }
                count += 1;
            }
        }
        out.push_str(raw);
    }
    if !done {
        return Err(ApiError::not_found("the cron entry no longer exists"));
    }
    Ok(out)
}

/// Install `text` as the user's crontab (`crontab <file>`: the runner has no stdin).
pub fn install_crontab(ctx: &Ctx, text: &str) -> Result<()> {
    let mut body = text.to_string();
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    let dir = ctx.env.data_dir.join("tmp");
    fs::create_dir_all(&dir)?;
    let file = dir.join(format!("crontab-{}.txt", random_id()));
    fs::write(&file, &body)?;
    let path = file.to_string_lossy().into_owned();
    let res = ctx.runner.run("crontab", &[&path]);
    let _ = fs::remove_file(&file);
    let out = res?;
    if out.success() {
        Ok(())
    } else {
        Err(ApiError::io(format!(
            "crontab rejected the new schedule: {}",
            summarize(&out)
        )))
    }
}
