//! Driver updater: find and install firmware / driver updates.
//!
//! Methods:
//! - `driver_updater.scan`: Linux `fwupdmgr` firmware updates and a recommended
//!   `ubuntu-drivers` package that is not installed; Windows Update driver updates (via the
//!   `Microsoft.Update.Session` COM API in PowerShell); macOS firmware items from
//!   `softwareupdate -l`.
//! - `driver_updater.backup`: Windows only, `pnputil /export-driver *` into
//!   `<data>/backups/drivers-<ts>`.
//! - `driver_updater.update { ids }`: installs the selected updates. Ids are matched against a
//!   fresh server-side scan. On Windows a backup is taken first and the update aborts if that
//!   fails.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

use crate::api::Registry;
use crate::ctx::{Ctx, Os};
use crate::elevate::{powershell_encode, run_privileged};
use crate::error::{ApiError, ErrorCode, Result};
use crate::features::software_updater::parse as su;
use crate::fsutil::now_unix;
use crate::job::{Job, ProgressEvent};
use crate::pkgutil::{summarize, valid_pkg_name};
use crate::runner::CmdOutput;

pub mod parse;
#[cfg(test)]
mod tests;

pub use parse::{DSource, DriverEntry};

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "driver_updater.scan",
    "driver_updater.backup",
    "driver_updater.update",
];

pub fn register(r: &mut Registry) {
    r.add("driver_updater.scan", scan_handler);
    r.add("driver_updater.backup", backup_handler);
    r.add("driver_updater.update", update_handler);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverResult {
    pub id: String,
    pub device_name: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub message: String,
    pub reboot_required: bool,
}

#[derive(Deserialize)]
struct UpdateParams {
    ids: Vec<String>,
}

// ---------------------------------------------------------------- scan

fn has(ctx: &Ctx, p: &str) -> bool {
    ctx.runner.which(p).is_some()
}

fn powershell(ctx: &Ctx, script: &str) -> Result<CmdOutput> {
    let enc = powershell_encode(script);
    ctx.runner.run(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &enc,
        ],
    )
}

fn scan_fwupd(ctx: &Ctx) -> Result<Vec<DriverEntry>> {
    let out = ctx.runner.run("fwupdmgr", &["get-updates", "--json"])?;
    match out.status {
        0 => parse::parse_fwupd(&out.stdout).map_err(ApiError::io),
        // FWUPD_EXIT_NOTHING_TO_DO: "No updatable devices" / "No updates available".
        2 => Ok(Vec::new()),
        n => {
            // Some versions print the JSON and still return non-zero; trust the JSON first.
            if let Ok(v) = parse::parse_fwupd(&out.stdout) {
                if !v.is_empty() {
                    return Ok(v);
                }
            }
            Err(ApiError::io(format!(
                "fwupdmgr get-updates failed (exit code {n}): {}",
                summarize(&out)
            )))
        }
    }
}

fn scan_ubuntu_drivers(ctx: &Ctx) -> Result<Vec<DriverEntry>> {
    let out = ctx.runner.run("ubuntu-drivers", &["devices"])?;
    if !out.success() {
        return Err(ApiError::io(format!(
            "ubuntu-drivers devices failed (exit code {}): {}",
            out.status,
            summarize(&out)
        )));
    }
    let mut v = Vec::new();
    for d in parse::parse_ubuntu_devices(&out.stdout) {
        // A recommended driver that is already installed needs no action.
        let installed = has(ctx, "dpkg-query")
            && ctx
                .runner
                .run(
                    "dpkg-query",
                    &["-W", "-f=${db:Status-Status}", &d.recommended],
                )
                .map(|o| o.success() && o.stdout.trim() == "installed")
                .unwrap_or(false);
        if installed {
            continue;
        }
        let name = if d.model.is_empty() {
            d.vendor.clone()
        } else {
            d.model.clone()
        };
        let mut e = DriverEntry::new(
            DSource::UbuntuDrivers,
            &d.recommended,
            &name,
            &format!(
                "Recommended proprietary driver {} is not installed",
                d.recommended
            ),
        );
        e.new_version = Some(d.recommended.clone());
        if !d.vendor.is_empty() {
            e.vendor = Some(d.vendor);
        }
        e.reboot_required = Some(true);
        v.push(e);
    }
    Ok(v)
}

fn scan_windows(ctx: &Ctx) -> Result<Vec<DriverEntry>> {
    let out = powershell(ctx, parse::WIN_SEARCH_SCRIPT)?;
    if !out.success() {
        return Err(ApiError::io(format!(
            "Windows Update driver search failed (exit code {}): {}",
            out.status,
            summarize(&out)
        )));
    }
    parse::parse_windows_drivers(&out.stdout).map_err(ApiError::io)
}

fn scan_macos(ctx: &Ctx) -> Result<Vec<DriverEntry>> {
    let out = ctx.runner.run("softwareupdate", &["-l"])?;
    if !out.success() {
        return Err(ApiError::io(format!(
            "softwareupdate -l failed (exit code {}): {}",
            out.status,
            summarize(&out)
        )));
    }
    let text = format!("{}\n{}", out.stdout, out.stderr);
    Ok(su::parse_softwareupdate(&text)
        .into_iter()
        .filter(|u| {
            let t = format!("{} {}", u.title, u.label).to_lowercase();
            t.contains("firmware")
        })
        .map(|u| {
            let mut e = DriverEntry::new(DSource::Macos, &u.label, &u.title, "Firmware update");
            e.new_version = (!u.version.is_empty()).then(|| u.version.clone());
            e.vendor = Some("Apple".into());
            e.reboot_required = Some(true); // firmware updates always restart the Mac
            e
        })
        .collect())
}

/// All driver / firmware updates available on this machine.
pub fn collect(ctx: &Ctx, job: &Job) -> Result<Vec<DriverEntry>> {
    let mut all: Vec<DriverEntry> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut tools = 0usize;
    let mut run = |name: &str, f: &dyn Fn(&Ctx) -> Result<Vec<DriverEntry>>| -> Result<()> {
        job.check_cancelled()?;
        job.progress(ProgressEvent::new("scan").message(format!("Checking {name}")));
        tools += 1;
        match f(ctx) {
            Ok(v) => all.extend(v),
            Err(e) if e.code == ErrorCode::Cancelled => return Err(e),
            Err(e) => errors.push(e.message),
        }
        Ok(())
    };
    match ctx.env.os {
        Os::Linux => {
            if has(ctx, "fwupdmgr") {
                run("firmware (fwupd)", &scan_fwupd)?;
            }
            if has(ctx, "ubuntu-drivers") {
                run("proprietary drivers", &scan_ubuntu_drivers)?;
            }
        }
        Os::Windows => {
            if has(ctx, "powershell") {
                run("Windows Update", &scan_windows)?;
            }
        }
        Os::MacOs => {
            if has(ctx, "softwareupdate") {
                run("firmware updates", &scan_macos)?;
            }
        }
    }
    if tools == 0 {
        return Err(ApiError::unsupported(match ctx.env.os {
            Os::Linux => "Neither fwupd (fwupdmgr) nor ubuntu-drivers is installed, so there is nothing to scan",
            Os::Windows => "PowerShell was not found",
            Os::MacOs => "softwareupdate was not found",
        }));
    }
    if all.is_empty() && !errors.is_empty() && errors.len() == tools {
        return Err(ApiError::io(errors.join("; ")));
    }
    all.sort_by(|a, b| {
        a.device_name
            .to_lowercase()
            .cmp(&b.device_name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(all)
}

fn scan_handler(ctx: &Ctx, _params: Value, job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(collect(ctx, job)?)?)
}

// ---------------------------------------------------------------- backup

fn require_windows_backup(ctx: &Ctx) -> Result<()> {
    if ctx.env.os != Os::Windows {
        return Err(ApiError::unsupported(
            "Driver backup is only available on Windows (Linux and macOS have no driver store to export)",
        ));
    }
    Ok(())
}

/// `pnputil /export-driver * <dir>` (elevated). Returns the backup folder.
pub fn backup_drivers(ctx: &Ctx, job: &Job) -> Result<(std::path::PathBuf, CmdOutput)> {
    let dir = ctx
        .env
        .data_dir
        .join("backups")
        .join(format!("drivers-{}", now_unix()));
    std::fs::create_dir_all(&dir)?;
    job.progress(ProgressEvent::new("backup").message("Backing up installed drivers".to_string()));
    let dir_s = dir.to_string_lossy().into_owned();
    let out = run_privileged(ctx, "pnputil", &["/export-driver", "*", &dir_s])?;
    Ok((dir, out))
}

fn backup_handler(ctx: &Ctx, _params: Value, job: &Job) -> Result<Value> {
    require_windows_backup(ctx)?;
    let (dir, out) = backup_drivers(ctx, job)?;
    let ok = out.success();
    Ok(serde_json::json!({
        "ok": ok,
        "path": dir.to_string_lossy(),
        "exitCode": out.status,
        "message": if ok { format!("Drivers exported to {}", dir.display()) } else { format!("Driver export failed (exit code {}): {}", out.status, summarize(&out)) },
    }))
}

// ---------------------------------------------------------------- update

fn label_ok(l: &str) -> bool {
    !l.is_empty() && l.len() <= 200 && !l.starts_with('-') && !l.chars().any(char::is_control)
}

fn reboot_mentioned(o: &CmdOutput) -> bool {
    let t = format!("{}\n{}", o.stdout, o.stderr).to_lowercase();
    t.contains("reboot") || t.contains("restart")
}

fn one_result(e: &DriverEntry, out: &CmdOutput, what: &str, force_reboot: bool) -> DriverResult {
    let ok = out.success();
    let s = summarize(out);
    DriverResult {
        id: e.id.clone(),
        device_name: e.device_name.clone(),
        ok,
        exit_code: Some(out.status),
        message: if ok {
            if s.is_empty() {
                format!("{what} finished")
            } else {
                s
            }
        } else if s.is_empty() {
            format!("{what} failed (exit code {})", out.status)
        } else {
            format!("{what} failed (exit code {}): {s}", out.status)
        },
        reboot_required: ok
            && (force_reboot || e.reboot_required == Some(true) || reboot_mentioned(out)),
    }
}

fn failed_result(e: &DriverEntry, message: &str) -> DriverResult {
    DriverResult {
        id: e.id.clone(),
        device_name: e.device_name.clone(),
        ok: false,
        exit_code: None,
        message: message.to_string(),
        reboot_required: false,
    }
}

fn install_windows(ctx: &Ctx, job: &Job, chosen: &[DriverEntry]) -> Result<Vec<DriverResult>> {
    let mut results = Vec::new();
    let keys: Vec<String> = chosen.iter().map(|e| e.key().to_string()).collect();
    let mut done = 0usize;
    for (chunk_entries, chunk_keys) in chosen.chunks(15).zip(keys.chunks(15)) {
        job.check_cancelled()?;
        job.progress(
            ProgressEvent::new("update")
                .counts(done as u64, chosen.len() as u64)
                .fraction(done as f64 / chosen.len() as f64)
                .message("Downloading and installing drivers".to_string()),
        );
        let script = parse::win_install_script(chunk_keys).map_err(ApiError::invalid_params)?;
        let enc = powershell_encode(&script);
        let out = run_privileged(
            ctx,
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-EncodedCommand",
                &enc,
            ],
        );
        let out = match out {
            Ok(o) => o,
            Err(e) if e.code == ErrorCode::PermissionDenied => {
                for en in chunk_entries {
                    results.push(failed_result(en, &e.message));
                }
                return Ok(results);
            }
            Err(e) => return Err(e),
        };
        if !out.success() {
            for en in chunk_entries {
                results.push(failed_result(
                    en,
                    &format!(
                        "Windows Update failed (exit code {}): {}",
                        out.status,
                        summarize(&out)
                    ),
                ));
            }
        } else {
            match parse::parse_windows_install(&out.stdout) {
                Ok(r) => {
                    for en in chunk_entries {
                        let key = en.key().to_lowercase();
                        match r.updates.iter().find(|u| u.0 == key) {
                            Some((_, code, hresult, reboot)) => {
                                let ok = *code == 2;
                                results.push(DriverResult {
                                    id: en.id.clone(),
                                    device_name: en.device_name.clone(),
                                    ok,
                                    exit_code: Some(*code as i32),
                                    message: match code {
                                        2 => "Installed".to_string(),
                                        3 => {
                                            format!("Installed with errors (HRESULT {hresult:#x})")
                                        }
                                        5 => "Aborted".to_string(),
                                        _ => format!("Installation failed (HRESULT {hresult:#x})"),
                                    },
                                    reboot_required: ok && (*reboot || r.reboot_required),
                                });
                            }
                            None => results.push(failed_result(
                                en,
                                "Windows Update did not report a result for this driver",
                            )),
                        }
                    }
                }
                Err(e) => {
                    for en in chunk_entries {
                        results.push(failed_result(en, &e));
                    }
                }
            }
        }
        done += chunk_entries.len();
    }
    Ok(results)
}

fn update_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: UpdateParams = serde_json::from_value(params)?;
    if p.ids.is_empty() {
        return Err(ApiError::invalid_params("`ids` is empty"));
    }
    if p.ids.len() > 500 {
        return Err(ApiError::invalid_params("too many ids"));
    }
    let available = collect(ctx, job)?;
    let wanted: HashSet<&str> = p.ids.iter().map(String::as_str).collect();
    let chosen: Vec<DriverEntry> = available
        .into_iter()
        .filter(|e| wanted.contains(e.id.as_str()))
        .collect();
    let known: HashSet<&str> = chosen.iter().map(|e| e.id.as_str()).collect();
    let mut results: Vec<DriverResult> = Vec::new();
    let mut seen = HashSet::new();
    for id in &p.ids {
        if !known.contains(id.as_str()) && seen.insert(id.clone()) {
            results.push(DriverResult {
                id: id.clone(),
                device_name: String::new(),
                ok: false,
                exit_code: None,
                message: "Refused: no update is available for this id".into(),
                reboot_required: false,
            });
        }
    }
    let mut backup_path: Option<String> = None;
    if ctx.env.os == Os::Windows && !chosen.is_empty() {
        // Never touch drivers without a way back.
        let (dir, out) = backup_drivers(ctx, job)?;
        if !out.success() {
            return Err(ApiError::io(format!(
                "Driver backup failed (exit code {}), so nothing was updated: {}",
                out.status,
                summarize(&out)
            )));
        }
        backup_path = Some(dir.to_string_lossy().into_owned());
    }
    match ctx.env.os {
        Os::Windows => {
            if !chosen.is_empty() {
                results.extend(install_windows(ctx, job, &chosen)?);
            }
        }
        _ => {
            let total = chosen.len() as u64;
            let mut aborted: Option<String> = None;
            for (i, e) in chosen.iter().enumerate() {
                if let Some(why) = &aborted {
                    results.push(failed_result(e, &format!("Not run: {why}")));
                    continue;
                }
                if job.is_cancelled() {
                    aborted = Some("cancelled".into());
                    results.push(failed_result(e, "Not run: cancelled"));
                    continue;
                }
                job.progress(
                    ProgressEvent::new("update")
                        .counts(i as u64, total)
                        .fraction(i as f64 / total.max(1) as f64)
                        .message(format!("Updating {}", e.device_name)),
                );
                let key = e.key();
                let run = match e.source {
                    DSource::Fwupd if valid_pkg_name(key) => {
                        run_privileged(ctx, "fwupdmgr", &["update", key, "-y", "--no-reboot-check"])
                            .map(|o| one_result(e, &o, "fwupdmgr update", false))
                    }
                    DSource::UbuntuDrivers if valid_pkg_name(key) => {
                        run_privileged(ctx, "ubuntu-drivers", &["install", key])
                            .map(|o| one_result(e, &o, "ubuntu-drivers install", true))
                    }
                    DSource::Macos if label_ok(key) => {
                        run_privileged(ctx, "softwareupdate", &["-i", key])
                            .map(|o| one_result(e, &o, "softwareupdate", true))
                    }
                    _ => Ok(failed_result(e, "Skipped: unsafe identifier")),
                };
                match run {
                    Ok(r) => results.push(r),
                    Err(err) => {
                        if err.code == ErrorCode::PermissionDenied {
                            aborted = Some(err.message.clone());
                        }
                        results.push(failed_result(e, &err.message));
                    }
                }
            }
            job.progress(
                ProgressEvent::new("update")
                    .counts(total, total)
                    .fraction(1.0),
            );
        }
    }
    results.sort_by(|a, b| a.id.cmp(&b.id));
    let reboot = results.iter().any(|r| r.reboot_required);
    let succeeded = results.iter().filter(|r| r.ok).count();
    let failed = results.len() - succeeded;
    Ok(serde_json::json!({
        "results": results,
        "rebootRequired": reboot,
        "succeeded": succeeded,
        "failed": failed,
        "backupPath": backup_path,
    }))
}
