//! Keep the operating system's scheduler in step with `schedules.json`.
//!
//! Every add / update / enable / disable calls [`sync`]; every delete calls [`remove`].
//! What is used depends on the OS (`ctx.env.os`, so all of it is unit-tested on any host):
//!
//! - Linux: systemd user units (`clearsweep-<id>.service` + `.timer`) when `systemctl --user`
//!   works, otherwise tagged lines in the user's crontab. `on_login` is an XDG autostart entry
//!   in both cases. Disabled schedules keep their unit files but the timer is disabled; with
//!   cron a disabled schedule simply has no line.
//! - Windows: `schtasks` tasks named `ClearSweep\<id>` (`/change /enable|/disable`). `on_login`
//!   falls back to an `HKCU\...\Run` value when creating a logon task is refused (it needs
//!   administrator rights on many systems).
//! - macOS: `~/Library/LaunchAgents/app.clearsweep.schedule.<id>.plist`, loaded with
//!   `launchctl bootstrap gui/<uid>`. A disabled schedule has no plist.
//!
//! Where nothing works ([`BackendKind::None`]) schedules are still stored, and the background
//! agent runs the ones that are due while it is running.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::ctx::{Ctx, Os};
use crate::error::{ApiError, Result};
use crate::features::startup::linux as xdg;
use crate::features::startup::model::WinHive;
use crate::features::startup::windows as win;
use crate::fsutil::atomic_write;
use crate::osjobs;
use crate::pkgutil::summarize;

use super::gen;
use super::model::{Frequency, Schedule};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Systemd,
    Cron,
    Schtasks,
    Launchd,
    /// Nothing usable: schedules are stored and run by the background agent only.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Backend {
    pub kind: BackendKind,
    pub available: bool,
    /// One sentence for the UI.
    pub detail: String,
}

/// Which mechanism runs schedules on this system, and whether it works.
pub fn detect(ctx: &Ctx) -> Backend {
    let (kind, available, detail) = match ctx.env.os {
        Os::Linux => {
            let systemd = ctx.runner.which("systemctl").is_some()
                && ctx
                    .runner
                    .run("systemctl", &["--user", "show-environment"])
                    .is_ok_and(|o| o.success());
            if systemd {
                (
                    BackendKind::Systemd,
                    true,
                    "Schedules run as systemd user timers.".to_string(),
                )
            } else if ctx.runner.which("crontab").is_some() {
                (
                    BackendKind::Cron,
                    true,
                    "systemd user services are not available; schedules run through cron."
                        .to_string(),
                )
            } else {
                (
                    BackendKind::None,
                    false,
                    "Neither systemd user services nor cron are available. Schedules run only while the ClearSweep background agent is running, or when you press Run now.".to_string(),
                )
            }
        }
        Os::Windows => {
            if ctx.runner.which("schtasks").is_some() {
                (
                    BackendKind::Schtasks,
                    true,
                    "Schedules run as Windows Task Scheduler tasks.".to_string(),
                )
            } else {
                (
                    BackendKind::None,
                    false,
                    "The Windows Task Scheduler (schtasks) was not found. Schedules run only while the ClearSweep background agent is running, or when you press Run now.".to_string(),
                )
            }
        }
        Os::MacOs => {
            if ctx.runner.which("launchctl").is_some() {
                (
                    BackendKind::Launchd,
                    true,
                    "Schedules run as launchd agents.".to_string(),
                )
            } else {
                (
                    BackendKind::None,
                    false,
                    "launchctl was not found. Schedules run only while the ClearSweep background agent is running, or when you press Run now.".to_string(),
                )
            }
        }
    };
    Backend {
        kind,
        available,
        detail,
    }
}

fn write_own(path: &Path, text: &str) -> Result<()> {
    atomic_write(path, text.as_bytes())
        .map_err(|e| ApiError::io(format!("could not write {}: {e}", path.display())))
}

fn remove_own(ctx: &Ctx, path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let dir = path
        .parent()
        .ok_or_else(|| ApiError::internal("path has no parent"))?;
    xdg::safe_remove_file(ctx, dir, path)
}

fn check(out: crate::runner::CmdOutput, what: &str) -> Result<()> {
    if out.success() {
        Ok(())
    } else {
        Err(ApiError::io(format!("{what} failed: {}", summarize(&out))))
    }
}

fn run(ctx: &Ctx, program: &str, args: &[String]) -> Result<crate::runner::CmdOutput> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    ctx.runner.run(program, &refs)
}

// ---------------------------------------------------------------- locations

pub fn systemd_dir(ctx: &Ctx) -> PathBuf {
    ctx.env.config_dir.join("systemd").join("user")
}
pub fn systemd_service_path(ctx: &Ctx, id: &str) -> PathBuf {
    systemd_dir(ctx).join(gen::systemd_service_name(id))
}
pub fn systemd_timer_path(ctx: &Ctx, id: &str) -> PathBuf {
    systemd_dir(ctx).join(gen::systemd_timer_name(id))
}
pub fn login_entry_path(ctx: &Ctx, id: &str) -> PathBuf {
    xdg::user_autostart_dir(ctx).join(gen::xdg_login_file(id))
}
pub fn launchd_plist_path(ctx: &Ctx, id: &str) -> PathBuf {
    ctx.env
        .home
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", gen::launchd_label(id)))
}

// ---------------------------------------------------------------- public API

/// Make the OS match `s` (install / update / enable / disable its job).
pub fn sync(ctx: &Ctx, exe: &Path, s: &Schedule) -> Result<()> {
    let exe = osjobs::check_exe(exe)?;
    match ctx.env.os {
        Os::Linux => sync_linux(ctx, &exe, s),
        Os::Windows => sync_windows(ctx, &exe, s),
        Os::MacOs => sync_macos(ctx, &exe, s),
    }
}

/// Remove every OS artifact of schedule `id`, whichever mechanism created it. Best effort:
/// returns what could not be cleaned up.
pub fn remove(ctx: &Ctx, id: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    if !super::model::valid_id(id) {
        return warnings;
    }
    let mut note = |r: Result<()>| {
        if let Err(e) = r {
            warnings.push(e.message);
        }
    };
    match ctx.env.os {
        Os::Linux => {
            note(remove_systemd(ctx, id));
            note(remove_cron(ctx, id));
            note(remove_own(ctx, &login_entry_path(ctx, id)));
        }
        Os::Windows => note(remove_windows(ctx, id)),
        Os::MacOs => note(remove_macos(ctx, id)),
    }
    warnings
}

// ---------------------------------------------------------------- Linux

fn sync_linux(ctx: &Ctx, exe: &str, s: &Schedule) -> Result<()> {
    if s.frequency == Frequency::OnLogin {
        let path = login_entry_path(ctx, &s.id);
        return if s.enabled {
            write_own(&path, &gen::login_desktop_entry(s, exe))
        } else {
            remove_own(ctx, &path)
        };
    }
    match detect(ctx).kind {
        BackendKind::Systemd => install_systemd(ctx, exe, s),
        BackendKind::Cron => install_cron(ctx, exe, s),
        // Nothing to talk to: the schedule is stored and the agent runs it.
        _ => Ok(()),
    }
}

fn install_systemd(ctx: &Ctx, exe: &str, s: &Schedule) -> Result<()> {
    let (Some(timer), service) = (gen::systemd_timer(s), gen::systemd_service(s, exe)) else {
        return Err(ApiError::internal("no timer for this schedule"));
    };
    let timer_path = systemd_timer_path(ctx, &s.id);
    let previous = fs::read_to_string(&timer_path).ok();
    write_own(&systemd_service_path(ctx, &s.id), &service)?;
    write_own(&timer_path, &timer)?;
    let unit = gen::systemd_timer_name(&s.id);
    check(
        ctx.runner.run("systemctl", &["--user", "daemon-reload"])?,
        "systemctl daemon-reload",
    )?;
    if s.enabled {
        check(
            ctx.runner
                .run("systemctl", &["--user", "enable", "--now", &unit])?,
            "systemctl enable",
        )?;
        if previous.is_some_and(|p| p != timer) {
            // A running timer keeps its old calendar until restarted.
            check(
                ctx.runner.run("systemctl", &["--user", "restart", &unit])?,
                "systemctl restart",
            )?;
        }
    } else {
        check(
            ctx.runner
                .run("systemctl", &["--user", "disable", "--now", &unit])?,
            "systemctl disable",
        )?;
    }
    Ok(())
}

fn remove_systemd(ctx: &Ctx, id: &str) -> Result<()> {
    let service = systemd_service_path(ctx, id);
    let timer = systemd_timer_path(ctx, id);
    if !service.exists() && !timer.exists() {
        return Ok(());
    }
    let unit = gen::systemd_timer_name(id);
    // Failing to disable (unit not loaded) must not stop the files from being removed.
    let _ = ctx
        .runner
        .run("systemctl", &["--user", "disable", "--now", &unit]);
    remove_own(ctx, &timer)?;
    remove_own(ctx, &service)?;
    let _ = ctx.runner.run("systemctl", &["--user", "daemon-reload"]);
    Ok(())
}

/// The user's crontab. A missing crontab is empty; any other failure is an error, so that
/// an unreadable crontab is never overwritten.
pub fn read_crontab_strict(ctx: &Ctx) -> Result<String> {
    let o = ctx.runner.run("crontab", &["-l"])?;
    if o.success() {
        return Ok(o.stdout);
    }
    let msg = format!("{} {}", o.stderr, o.stdout).to_lowercase();
    if msg.contains("no crontab") {
        Ok(String::new())
    } else {
        Err(ApiError::io(format!(
            "could not read the crontab: {}",
            summarize(&o)
        )))
    }
}

fn install_cron(ctx: &Ctx, exe: &str, s: &Schedule) -> Result<()> {
    let text = read_crontab_strict(ctx)?;
    let line = if s.enabled {
        gen::cron_line(s, exe)
    } else {
        None
    };
    let next = gen::cron_upsert(&text, &s.id, line.as_deref());
    if next != text {
        xdg::install_crontab(ctx, &next)?;
    }
    Ok(())
}

fn remove_cron(ctx: &Ctx, id: &str) -> Result<()> {
    if ctx.runner.which("crontab").is_none() {
        return Ok(());
    }
    let text = read_crontab_strict(ctx)?;
    if !gen::cron_has(&text, id) {
        return Ok(());
    }
    xdg::install_crontab(ctx, &gen::cron_upsert(&text, id, None))
}

// ---------------------------------------------------------------- Windows

fn win_run_key() -> String {
    format!(r"{}\Software\{}", WinHive::Hkcu.name(), win::RUN)
}

fn sync_windows(ctx: &Ctx, exe: &str, s: &Schedule) -> Result<()> {
    if ctx.runner.which("schtasks").is_none() {
        return Ok(());
    }
    let create = run(ctx, "schtasks", &gen::schtasks_create_args(s, exe))?;
    if !create.success() {
        if s.frequency == Frequency::OnLogin {
            return win_login_fallback(ctx, exe, s, &create);
        }
        return check(create, "schtasks /create");
    }
    if s.frequency == Frequency::OnLogin {
        // A stale Run value from an earlier fallback would run the job twice.
        let _ = win_delete_run_value(ctx, &s.id);
    }
    if !s.enabled {
        check(
            run(ctx, "schtasks", &gen::schtasks_change_args(&s.id, false))?,
            "schtasks /change /disable",
        )?;
    }
    Ok(())
}

/// Creating a logon task is refused without administrator rights: use the per-user Run key.
fn win_login_fallback(
    ctx: &Ctx,
    exe: &str,
    s: &Schedule,
    refused: &crate::runner::CmdOutput,
) -> Result<()> {
    let name = gen::win_run_value(&s.id);
    if !s.enabled {
        return win_delete_run_value(ctx, &s.id);
    }
    let data = osjobs::windows_command(exe, &osjobs::schedule_args(&s.id));
    let out = ctx.runner.run(
        "reg",
        &[
            "add",
            &win_run_key(),
            "/v",
            &name,
            "/t",
            "REG_SZ",
            "/d",
            &data,
            "/f",
        ],
    )?;
    if out.success() {
        Ok(())
    } else {
        Err(ApiError::io(format!(
            "schtasks /create failed ({}) and so did the Run-key fallback: {}",
            summarize(refused),
            summarize(&out)
        )))
    }
}

fn win_delete_run_value(ctx: &Ctx, id: &str) -> Result<()> {
    let name = gen::win_run_value(id);
    let key = win_run_key();
    let q = ctx.runner.run("reg", &["query", &key, "/v", &name]);
    if !matches!(q, Ok(ref o) if o.success()) {
        return Ok(()); // nothing there
    }
    check(
        ctx.runner
            .run("reg", &["delete", &key, "/v", &name, "/f"])?,
        "reg delete",
    )
}

fn remove_windows(ctx: &Ctx, id: &str) -> Result<()> {
    let mut first: Option<ApiError> = None;
    if ctx.runner.which("schtasks").is_some() {
        match run(ctx, "schtasks", &gen::schtasks_delete_args(id)) {
            // "The system cannot find the file specified": nothing to delete.
            Ok(o) if !o.success() => {
                let msg = format!("{} {}", o.stderr, o.stdout).to_lowercase();
                if !(msg.contains("cannot find") || msg.contains("does not exist")) {
                    first = Some(ApiError::io(format!(
                        "schtasks /delete failed: {}",
                        summarize(&o)
                    )));
                }
            }
            Ok(_) => {}
            Err(e) => first = Some(e),
        }
    }
    let run_value = win_delete_run_value(ctx, id);
    match (first, run_value) {
        (Some(e), _) => Err(e),
        (None, r) => r,
    }
}

// ---------------------------------------------------------------- macOS

fn sync_macos(ctx: &Ctx, exe: &str, s: &Schedule) -> Result<()> {
    if ctx.runner.which("launchctl").is_none() {
        return Ok(());
    }
    let path = launchd_plist_path(ctx, &s.id);
    let domain = crate::autostart::mac_domain(ctx)?;
    let target = format!("{domain}/{}", gen::launchd_label(&s.id));
    if s.enabled {
        write_own(&path, &gen::launchd_plist(s, exe))?;
        let _ = ctx.runner.run("launchctl", &["bootout", &target]);
        let p = path.to_string_lossy().into_owned();
        check(
            ctx.runner.run("launchctl", &["bootstrap", &domain, &p])?,
            "launchctl bootstrap",
        )
    } else {
        let _ = ctx.runner.run("launchctl", &["bootout", &target]);
        remove_own(ctx, &path)
    }
}

fn remove_macos(ctx: &Ctx, id: &str) -> Result<()> {
    let path = launchd_plist_path(ctx, id);
    if ctx.runner.which("launchctl").is_some() {
        if let Ok(domain) = crate::autostart::mac_domain(ctx) {
            let target = format!("{domain}/{}", gen::launchd_label(id));
            let _ = ctx.runner.run("launchctl", &["bootout", &target]);
        }
    }
    remove_own(ctx, &path)
}
