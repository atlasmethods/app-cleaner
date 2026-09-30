//! "Run at startup": an OS autostart entry that launches `<job_exe> agent`, the headless
//! background agent (smart cleaning, sleep-mode enforcement, scheduled-clean catch-up).
//!
//! The desktop app is deliberately NOT what autostart launches: the agent needs no window
//! and no tray, and it holds the single-instance lock, so opening the desktop app later just
//! finds the agent already running.
//!
//! - Linux: `~/.config/autostart/clearsweep-agent.desktop`
//! - Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value `ClearSweepAgent`
//! - macOS: `~/Library/LaunchAgents/app.clearsweep.agent.plist` (+ `launchctl bootstrap`)
//!
//! [`apply`] is idempotent. `settings.set { runAtStartup }` calls it and refuses to save the
//! setting when it fails; `settings.get` calls [`is_installed`] so the reported value always
//! reflects reality.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ctx::{Ctx, Os};
use crate::error::{ApiError, Result};
use crate::features::startup::linux as xdg;
use crate::features::startup::model::{RunLoc, WinHive};
use crate::features::startup::windows as win;
use crate::fsutil::atomic_write;
use crate::osjobs::{self, LaunchTrigger};
use crate::pkgutil::summarize;

pub const LINUX_FILE: &str = "clearsweep-agent.desktop";
pub const MAC_LABEL: &str = "app.clearsweep.agent";
pub const WIN_VALUE: &str = "ClearSweepAgent";

pub fn linux_path(ctx: &Ctx) -> PathBuf {
    xdg::user_autostart_dir(ctx).join(LINUX_FILE)
}

pub fn mac_path(ctx: &Ctx) -> PathBuf {
    ctx.env
        .home
        .join("Library/LaunchAgents")
        .join(format!("{MAC_LABEL}.plist"))
}

fn win_run_key() -> String {
    format!(r"{}\Software\{}", WinHive::Hkcu.name(), win::RUN)
}

/// The launchd GUI domain of the current user, `gui/<uid>`.
pub fn mac_domain(ctx: &Ctx) -> Result<String> {
    let o = ctx.runner.run("id", &["-u"])?;
    let uid = o.stdout.trim();
    if o.success() && !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit()) {
        Ok(format!("gui/{uid}"))
    } else {
        Err(ApiError::io("could not determine the user id (id -u)"))
    }
}

/// Install (`enabled`) or remove the autostart entry for the running executable.
pub fn apply(ctx: &Ctx, enabled: bool) -> Result<()> {
    apply_with_exe(ctx, enabled, &crate::exe::job_exe())
}

pub fn apply_with_exe(ctx: &Ctx, enabled: bool, exe: &Path) -> Result<()> {
    match ctx.env.os {
        Os::Linux => apply_linux(ctx, enabled, exe),
        Os::Windows => apply_windows(ctx, enabled, exe),
        Os::MacOs => apply_macos(ctx, enabled, exe),
    }
}

/// Is the autostart entry present and active? `None` when it cannot be determined.
pub fn is_installed(ctx: &Ctx) -> Option<bool> {
    match ctx.env.os {
        Os::Linux => Some(
            fs::read_to_string(linux_path(ctx))
                .map(|t| xdg::parse_desktop(&t).enabled())
                .unwrap_or(false),
        ),
        Os::MacOs => Some(mac_path(ctx).is_file()),
        Os::Windows => {
            ctx.runner.which("reg")?;
            let out = ctx
                .runner
                .run("reg", &["query", &win_run_key(), "/v", WIN_VALUE])
                .ok()?;
            // `reg query` exits 1 when the value does not exist.
            match out.status {
                0 => Some(true),
                1 => Some(false),
                _ => None,
            }
        }
    }
}

// ---------------------------------------------------------------- Linux

fn remove_own_file(ctx: &Ctx, path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let dir = path
        .parent()
        .ok_or_else(|| ApiError::internal("autostart path has no parent"))?;
    xdg::safe_remove_file(ctx, dir, path)
}

fn write_own_file(path: &Path, text: &str) -> Result<()> {
    atomic_write(path, text.as_bytes())
        .map_err(|e| ApiError::io(format!("could not write {}: {e}", path.display())))
}

/// The `.desktop` text of the agent's autostart entry.
pub fn linux_entry(exe: &Path) -> Result<String> {
    let exe = osjobs::check_exe(exe)?;
    Ok(osjobs::desktop_entry(
        "ClearSweep background agent",
        "Smart cleaning and scheduled maintenance for ClearSweep",
        &exe,
        &osjobs::agent_args(),
    ))
}

fn apply_linux(ctx: &Ctx, enabled: bool, exe: &Path) -> Result<()> {
    let path = linux_path(ctx);
    if enabled {
        write_own_file(&path, &linux_entry(exe)?)
    } else {
        remove_own_file(ctx, &path)
    }
}

// ---------------------------------------------------------------- macOS

pub fn mac_plist(exe: &Path) -> Result<String> {
    let exe = osjobs::check_exe(exe)?;
    Ok(osjobs::launchd_plist(
        MAC_LABEL,
        &exe,
        &osjobs::agent_args(),
        &LaunchTrigger::RunAtLoad,
    ))
}

fn apply_macos(ctx: &Ctx, enabled: bool, exe: &Path) -> Result<()> {
    let path = mac_path(ctx);
    let domain = mac_domain(ctx)?;
    let target = format!("{domain}/{MAC_LABEL}");
    if enabled {
        write_own_file(&path, &mac_plist(exe)?)?;
        // Replace a loaded copy; "not loaded" is an error we ignore.
        let _ = ctx.runner.run("launchctl", &["bootout", &target]);
        let p = path.to_string_lossy().into_owned();
        let out = ctx.runner.run("launchctl", &["bootstrap", &domain, &p])?;
        if !out.success() {
            return Err(ApiError::io(format!(
                "launchctl bootstrap failed: {}",
                summarize(&out)
            )));
        }
        Ok(())
    } else {
        let _ = ctx.runner.run("launchctl", &["bootout", &target]);
        remove_own_file(ctx, &path)
    }
}

// ---------------------------------------------------------------- Windows

fn apply_windows(ctx: &Ctx, enabled: bool, exe: &Path) -> Result<()> {
    let key = win_run_key();
    if enabled {
        let exe = osjobs::check_exe(exe)?;
        let data = osjobs::windows_command(&exe, &osjobs::agent_args());
        let out = ctx.runner.run(
            "reg",
            &[
                "add", &key, "/v", WIN_VALUE, "/t", "REG_SZ", "/d", &data, "/f",
            ],
        )?;
        if !out.success() {
            return Err(ApiError::io(format!("reg add failed: {}", summarize(&out))));
        }
        // A "disabled" flag left by the startup manager would keep the entry off.
        let _ = win::write_approved(ctx, WinHive::Hkcu, "Run", WIN_VALUE, true);
        Ok(())
    } else {
        // Idempotent: a value that is already gone (or no `reg` to look with) is fine.
        if ctx.runner.which("reg").is_none() || is_installed(ctx) == Some(false) {
            return Ok(());
        }
        let loc = RunLoc {
            hive: WinHive::Hkcu,
            key: format!(r"Software\{}", win::RUN),
            approved: Some("Run"),
        };
        win::delete_run_value(ctx, &loc, WIN_VALUE)
    }
}

#[cfg(test)]
#[path = "autostart_tests.rs"]
mod tests;
