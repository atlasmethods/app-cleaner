//! Uninstall: list installed applications, remove them, and clean up what they leave behind.
//!
//! Methods:
//! - `uninstall.list`: installed applications (dpkg/rpm/pacman/flatpak/snap/AppImage,
//!   the Windows `Uninstall` registry keys, macOS `.app` bundles, Homebrew).
//! - `uninstall.run { id, force? }`: uninstall through the platform's own tool (elevated
//!   when needed) and scan for leftovers afterwards. System components need `force`; a
//!   package whose removal would drag others along is reported (`needsForce`) unless forced.
//! - `uninstall.repair { id }` (Windows), `uninstall.remove_entry { id }` and
//!   `uninstall.rename_entry { id, name }` (Windows; the key is exported to
//!   `<data>/backups/uninstall-<ts>.reg` first).
//! - `uninstall.leftovers { name, id, bundleId? }` / `uninstall.remove_leftovers { name, id,
//!   bundleId?, paths }`: removal re-scans on the server and only deletes the intersection,
//!   and only for applications ClearSweep itself uninstalled (see [`ledger`]).
//!
//! Ids are the ones returned by `uninstall.list`; the client never supplies commands or paths
//! for the uninstall itself (the entry is looked up again server-side).

use serde::Deserialize;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::api::Registry;
use crate::ctx::{Ctx, Os};
use crate::elevate::run_privileged;
use crate::error::{ApiError, Result};
use crate::fsutil::now_unix;
use crate::job::{Job, ProgressEvent};
use crate::pkgutil::summarize;
use crate::runner::CmdOutput;

pub mod ledger;
pub mod leftovers;
pub mod linux;
pub mod macos;
pub mod model;
pub mod windows;

#[cfg(test)]
mod tests;

use model::{Action, Found, Hive, WinAction};
pub use model::{AppEntry, Leftover, RunResult, Source};
use windows::UninstallRegistry;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "uninstall.list",
    "uninstall.run",
    "uninstall.repair",
    "uninstall.remove_entry",
    "uninstall.rename_entry",
    "uninstall.leftovers",
    "uninstall.remove_leftovers",
];

pub fn register(r: &mut Registry) {
    r.add("uninstall.list", list_handler);
    r.add("uninstall.run", run_handler);
    r.add("uninstall.repair", repair_handler);
    r.add("uninstall.remove_entry", remove_entry_handler);
    r.add("uninstall.rename_entry", rename_entry_handler);
    r.add("uninstall.leftovers", leftovers_handler);
    r.add("uninstall.remove_leftovers", remove_leftovers_handler);
}

// ---------------------------------------------------------------- registry access

type RegHandle = Rc<dyn UninstallRegistry>;

thread_local! {
    static REG_OVERRIDE: RefCell<Option<RegHandle>> = const { RefCell::new(None) };
}

/// Run `f` with `reg` standing in for the Windows registry on this thread (tests only).
#[cfg(test)]
pub fn with_registry<T>(reg: impl UninstallRegistry + 'static, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<RegHandle>);
    impl Drop for Restore {
        fn drop(&mut self) {
            REG_OVERRIDE.with(|r| *r.borrow_mut() = self.0.take());
        }
    }
    let prev = REG_OVERRIDE.with(|r| r.borrow_mut().replace(Rc::new(reg)));
    let _restore = Restore(prev);
    f()
}

fn registry_handle() -> Result<RegHandle> {
    if let Some(r) = REG_OVERRIDE.with(|r| r.borrow().clone()) {
        return Ok(r);
    }
    #[cfg(windows)]
    {
        Ok(Rc::new(windows::RealRegistry))
    }
    #[cfg(not(windows))]
    {
        Err(ApiError::unsupported(
            "The Windows registry is not available on this system",
        ))
    }
}

// ---------------------------------------------------------------- collection

fn run_ok(ctx: &Ctx, program: &str, args: &[&str]) -> Option<String> {
    ctx.runner
        .run(program, args)
        .ok()
        .filter(|o| o.success())
        .map(|o| o.stdout)
}

/// Everything installed on this machine, sorted by name. A tool that is missing or fails
/// simply contributes nothing.
pub fn collect(ctx: &Ctx, job: &Job) -> Result<Vec<Found>> {
    collect_with(ctx, job, true)
}

/// `sizes = false` skips the (slow) size measurement of macOS app bundles; used when an entry is
/// only being looked up.
fn collect_with(ctx: &Ctx, job: &Job, sizes: bool) -> Result<Vec<Found>> {
    let mut all: Vec<Found> = Vec::new();
    let step = |msg: &str| {
        job.progress(ProgressEvent::new("list").message(msg.to_string()));
    };
    match ctx.env.os {
        Os::Linux => {
            job.check_cancelled()?;
            if ctx.runner.which("dpkg-query").is_some() {
                step("Reading dpkg packages");
                if let Some(o) = run_ok(ctx, "dpkg-query", &["-W", linux::DPKG_FORMAT]) {
                    all.extend(linux::parse_dpkg(&o, ctx));
                }
            }
            if ctx.runner.which("rpm").is_some() {
                step("Reading rpm packages");
                if let Some(o) = run_ok(ctx, "rpm", &["-qa", "--queryformat", linux::RPM_FORMAT]) {
                    all.extend(linux::parse_rpm(&o));
                }
            }
            if ctx.runner.which("pacman").is_some() {
                step("Reading pacman packages");
                if let Some(o) = run_ok(ctx, "env", &["LC_ALL=C", "pacman", "-Qi"]) {
                    all.extend(linux::parse_pacman(&o));
                }
            }
            job.check_cancelled()?;
            if ctx.runner.which("flatpak").is_some() {
                step("Reading Flatpak apps");
                if let Some(o) = run_ok(ctx, "flatpak", &["list", "--app", linux::FLATPAK_COLUMNS])
                {
                    all.extend(linux::parse_flatpak(&o));
                }
            }
            if ctx.runner.which("snap").is_some() {
                step("Reading Snap packages");
                if let Some(o) = run_ok(ctx, "snap", &["list"]) {
                    all.extend(linux::parse_snap(&o));
                }
            }
            step("Looking for AppImages");
            all.extend(linux::scan_appimages(ctx));
            collect_brew(ctx, &mut all);
        }
        Os::MacOs => {
            step("Reading applications");
            all.extend(macos::scan_apps(ctx, sizes));
            job.check_cancelled()?;
            collect_brew(ctx, &mut all);
        }
        Os::Windows => {
            step("Reading installed programs");
            let reg = registry_handle()?;
            all.extend(windows::parse_entries(&reg.entries()?));
        }
    }
    job.check_cancelled()?;
    all.sort_by(|a, b| {
        a.entry
            .name
            .to_lowercase()
            .cmp(&b.entry.name.to_lowercase())
            .then_with(|| a.entry.id.cmp(&b.entry.id))
    });
    Ok(all)
}

fn collect_brew(ctx: &Ctx, all: &mut Vec<Found>) {
    if ctx.runner.which("brew").is_none() {
        return;
    }
    if let Some(o) = run_ok(ctx, "brew", &["list", "--versions"]) {
        all.extend(linux::parse_brew(&o, false));
    }
    if let Some(o) = run_ok(ctx, "brew", &["list", "--cask", "--versions"]) {
        all.extend(linux::parse_brew(&o, true));
    }
}

fn find(ctx: &Ctx, job: &Job, id: &str) -> Result<Found> {
    if id.trim().is_empty() || id.len() > 1000 {
        return Err(ApiError::invalid_params("`id` is missing or too long"));
    }
    collect_with(ctx, job, false)?
        .into_iter()
        .find(|f| f.entry.id == id)
        .ok_or_else(|| ApiError::not_found(format!("`{id}` is not installed (any more)")))
}

// ---------------------------------------------------------------- params

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdParams {
    id: String,
    #[serde(default)]
    force: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameParams {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LeftoverParams {
    name: String,
    id: String,
    #[serde(default)]
    bundle_id: Option<String>,
    #[serde(default)]
    paths: Vec<String>,
}

// ---------------------------------------------------------------- list

fn list_handler(ctx: &Ctx, _params: Value, job: &Job) -> Result<Value> {
    let apps: Vec<AppEntry> = collect(ctx, job)?.into_iter().map(|f| f.entry).collect();
    Ok(serde_json::to_value(apps)?)
}

// ---------------------------------------------------------------- run

fn base_result(f: &Found) -> RunResult {
    RunResult {
        id: f.entry.id.clone(),
        name: f.entry.name.clone(),
        ok: false,
        exit_code: None,
        message: String::new(),
        also_removes: Vec::new(),
        needs_force: false,
        reboot_required: false,
        leftovers: Vec::new(),
        bundle_id: None,
    }
}

fn finish(mut r: RunResult, out: &CmdOutput, ok_codes: &[i32], what: &str) -> RunResult {
    r.exit_code = Some(out.status);
    r.ok = ok_codes.contains(&out.status);
    r.reboot_required = matches!(out.status, 3010 | 1641);
    let s = summarize(out);
    r.message = if r.ok {
        if s.is_empty() {
            format!("{what} finished")
        } else {
            s
        }
    } else if s.is_empty() {
        format!("{what} failed (exit code {})", out.status)
    } else {
        format!("{what} failed (exit code {}): {s}", out.status)
    };
    r
}

/// Packages `apt-get -s remove <pkg>` would remove besides `pkg` itself.
pub fn apt_dependents(sim: &str, pkg: &str) -> Vec<String> {
    let arch_prefix = format!("{pkg}:");
    sim.lines()
        .filter_map(|l| l.strip_prefix("Remv "))
        .filter_map(|l| l.split_whitespace().next())
        .filter(|n| *n != pkg && !n.starts_with(&arch_prefix))
        .map(str::to_string)
        .collect()
}

/// `rpm -q --whatrequires` output -> package names (empty when nothing requires it).
pub fn rpm_dependents(out: &CmdOutput) -> Vec<String> {
    if !out.success() {
        return Vec::new();
    }
    out.stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("no package requires"))
        .map(str::to_string)
        .collect()
}

fn check_dependents(ctx: &Ctx, action: &Action) -> Vec<String> {
    match action {
        Action::Dpkg(pkg) => ctx
            .runner
            .run("apt-get", &["-s", "remove", pkg])
            .ok()
            .filter(|o| o.success())
            .map(|o| apt_dependents(&o.stdout, pkg))
            .unwrap_or_default(),
        Action::Rpm(pkg) => ctx
            .runner
            .run("rpm", &["-q", "--whatrequires", pkg])
            .ok()
            .map(|o| rpm_dependents(&o))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn run_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: IdParams = serde_json::from_value(params)?;
    let found = find(ctx, job, &p.id)?;
    let mut r = base_result(&found);
    let e = &found.entry;
    if !e.uninstallable {
        return Err(ApiError::invalid_params(format!(
            "`{}` cannot be uninstalled (no uninstaller is registered for it)",
            e.name
        )));
    }
    if e.is_system && !p.force {
        return Err(ApiError::permission_denied(format!(
            "`{}` is a system component; removing it may break your system. \
             Confirm explicitly to remove it anyway.",
            e.name
        )));
    }
    if !p.force {
        let deps = check_dependents(ctx, &found.action);
        if !deps.is_empty() {
            r.needs_force = true;
            r.also_removes = deps;
            r.message = format!(
                "Removing `{}` would also remove {} other package(s).",
                e.name,
                r.also_removes.len()
            );
            return Ok(serde_json::to_value(r)?);
        }
    }
    job.progress(ProgressEvent::new("uninstall").message(format!("Uninstalling {}", e.name)));
    job.check_cancelled()?;

    let name = e.name.clone();
    let id = e.id.clone();
    let mut bundle_id: Option<String> = None;
    let mut res = match &found.action {
        Action::Dpkg(pkg) => {
            if ctx.runner.which("apt-get").is_some() {
                let out = run_privileged(ctx, "apt-get", &["remove", "-y", pkg])?;
                finish(r, &out, &[0], "apt-get remove")
            } else {
                let out = run_privileged(ctx, "dpkg", &["-r", pkg])?;
                finish(r, &out, &[0], "dpkg -r")
            }
        }
        Action::Rpm(pkg) => {
            if ctx.runner.which("dnf").is_some() || ctx.runner.which("rpm").is_none() {
                let out = run_privileged(ctx, "dnf", &["remove", "-y", pkg])?;
                finish(r, &out, &[0], "dnf remove")
            } else {
                let out = run_privileged(ctx, "rpm", &["-e", pkg])?;
                finish(r, &out, &[0], "rpm -e")
            }
        }
        Action::Pacman(pkg) => {
            let out = run_privileged(ctx, "pacman", &["-R", "--noconfirm", pkg])?;
            finish(r, &out, &[0], "pacman -R")
        }
        Action::Flatpak(app) => {
            let out = ctx.runner.run("flatpak", &["uninstall", "-y", app])?;
            finish(r, &out, &[0], "flatpak uninstall")
        }
        Action::Snap(pkg) => {
            let out = run_privileged(ctx, "snap", &["remove", pkg])?;
            finish(r, &out, &[0], "snap remove")
        }
        Action::AppImage(path) => {
            let (bytes, removed) = leftovers::remove_appimage(ctx, path, job)?;
            r.ok = true;
            r.message = format!(
                "Removed {} ({} freed){}",
                path.display(),
                crate::pkgutil::human_bytes(bytes),
                if removed.is_empty() {
                    String::new()
                } else {
                    format!(
                        " and {} launcher entr{}",
                        removed.len(),
                        if removed.len() == 1 { "y" } else { "ies" }
                    )
                }
            );
            r
        }
        Action::Windows(w) => {
            let (prog, args) = windows::uninstall_command(w).ok_or_else(|| {
                ApiError::invalid_params(format!("`{}` has no usable uninstall command", e.name))
            })?;
            let argv: Vec<&str> = args.iter().map(String::as_str).collect();
            let out = if w.hive.needs_admin() {
                run_privileged(ctx, &prog, &argv)?
            } else {
                ctx.runner.run(&prog, &argv)?
            };
            finish(r, &out, &[0, 3010, 1641], "The uninstaller")
        }
        Action::MacApp(path) => {
            let path = macos::validate_app_path(ctx, path).map_err(ApiError::invalid_params)?;
            bundle_id = macos::bundle_id_of(&path);
            trash_mac_app(ctx, &path, r)?
        }
        Action::Brew { name, cask } => {
            let mut args = vec!["uninstall"];
            if *cask {
                args.push("--cask");
            }
            args.push(name);
            let out = ctx.runner.run("brew", &args)?;
            finish(r, &out, &[0], "brew uninstall")
        }
    };
    if res.ok {
        res.bundle_id = bundle_id.clone();
        ledger::record(ctx, &id, &name, bundle_id.as_deref());
        job.progress(ProgressEvent::new("leftovers").message("Looking for leftovers".to_string()));
        res.leftovers = leftovers::scan(ctx, &name, &id, bundle_id.as_deref());
    }
    Ok(serde_json::to_value(res)?)
}

fn trash_mac_app(ctx: &Ctx, path: &std::path::Path, mut r: RunResult) -> Result<RunResult> {
    let script = macos::trash_script(path);
    let out = ctx.runner.run("osascript", &["-e", &script]);
    match out {
        Ok(o) if o.success() => {
            r.ok = true;
            r.exit_code = Some(0);
            r.message = format!("Moved {} to the Trash", path.display());
            Ok(r)
        }
        other => {
            // Finder refused (or is unavailable): a per-user app can be moved by hand.
            let home_apps = ctx.env.home.join("Applications");
            if path.parent() == crate::safety::canonicalize(&home_apps).ok().as_deref() {
                let trash = ctx.env.home.join(".Trash");
                std::fs::create_dir_all(&trash)?;
                let file = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let mut dest = trash.join(&file);
                let mut n = 1;
                while dest.exists() {
                    dest = trash.join(format!("{file} {n}"));
                    n += 1;
                }
                std::fs::rename(path, &dest)?;
                r.ok = true;
                r.exit_code = Some(0);
                r.message = format!("Moved {} to the Trash", path.display());
                return Ok(r);
            }
            match other {
                Ok(o) => Ok(finish(r, &o, &[0], "Moving to the Trash")),
                Err(e) => Err(e),
            }
        }
    }
}

// ---------------------------------------------------------------- Windows-only actions

fn require_windows(ctx: &Ctx, what: &str) -> Result<()> {
    if ctx.env.os != Os::Windows {
        return Err(ApiError::unsupported(format!(
            "{what} is only available on Windows"
        )));
    }
    Ok(())
}

fn windows_entry(ctx: &Ctx, job: &Job, id: &str) -> Result<(Found, WinAction)> {
    let f = find(ctx, job, id)?;
    match &f.action {
        Action::Windows(w) => {
            let w = w.clone();
            Ok((f, w))
        }
        _ => Err(ApiError::invalid_params("not a Windows program entry")),
    }
}

fn repair_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: IdParams = serde_json::from_value(params)?;
    require_windows(ctx, "Repair")?;
    let (found, w) = windows_entry(ctx, job, &p.id)?;
    if !found.entry.can_repair {
        return Err(ApiError::invalid_params(format!(
            "`{}` cannot be repaired",
            found.entry.name
        )));
    }
    let (prog, args) = windows::repair_command(&w)
        .ok_or_else(|| ApiError::invalid_params("no repair command is registered"))?;
    job.progress(ProgressEvent::new("repair").message(format!("Repairing {}", found.entry.name)));
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = if w.hive.needs_admin() {
        run_privileged(ctx, &prog, &argv)?
    } else {
        ctx.runner.run(&prog, &argv)?
    };
    let r = finish(base_result(&found), &out, &[0, 3010, 1641], "The repair");
    Ok(serde_json::to_value(r)?)
}

fn backup_file(ctx: &Ctx) -> Result<PathBuf> {
    let dir = ctx.env.data_dir.join("backups");
    std::fs::create_dir_all(&dir)?;
    let ts = now_unix();
    for n in 0..1000u32 {
        let name = if n == 0 {
            format!("uninstall-{ts}.reg")
        } else {
            format!("uninstall-{ts}-{n}.reg")
        };
        let p = dir.join(name);
        if !p.exists() {
            return Ok(p);
        }
    }
    Err(ApiError::io("could not choose a backup file name"))
}

/// Export the entry's registry key to a `.reg` file; nothing is modified when this fails.
fn backup_key(ctx: &Ctx, w: &WinAction) -> Result<PathBuf> {
    if !windows::key_name_ok(&w.key_name) {
        return Err(ApiError::invalid_params("unsafe registry key name"));
    }
    let file = backup_file(ctx)?;
    let reg_path = w.hive.reg_path(&w.key_name);
    let file_s = file.to_string_lossy().into_owned();
    let out = ctx
        .runner
        .run("reg", &["export", &reg_path, &file_s, "/y"])?;
    if !out.success() {
        return Err(ApiError::io(format!(
            "could not back up the registry key first: {}",
            summarize(&out)
        )));
    }
    Ok(file)
}

fn reg_command(ctx: &Ctx, hive: Hive, args: &[&str]) -> Result<CmdOutput> {
    if hive.needs_admin() {
        run_privileged(ctx, "reg", args)
    } else {
        ctx.runner.run("reg", args)
    }
}

fn remove_entry_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: IdParams = serde_json::from_value(params)?;
    require_windows(ctx, "Removing a registry entry")?;
    let (found, w) = windows_entry(ctx, job, &p.id)?;
    let backup = backup_key(ctx, &w)?;
    let reg_path = w.hive.reg_path(&w.key_name);
    let out = reg_command(ctx, w.hive, &["delete", &reg_path, "/f"])?;
    let r = finish(base_result(&found), &out, &[0], "reg delete");
    let mut v = serde_json::to_value(r)?;
    v["backupPath"] = json!(backup.to_string_lossy());
    Ok(v)
}

fn rename_entry_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: RenameParams = serde_json::from_value(params)?;
    require_windows(ctx, "Renaming a registry entry")?;
    let name = p.name.trim();
    if name.is_empty() || name.chars().count() > 200 || !windows::reg_arg_ok(name) {
        return Err(ApiError::invalid_params(
            "the new name must be 1-200 characters without quotes or percent signs",
        ));
    }
    let (found, w) = windows_entry(ctx, job, &p.id)?;
    let backup = backup_key(ctx, &w)?;
    let reg_path = w.hive.reg_path(&w.key_name);
    let out = reg_command(
        ctx,
        w.hive,
        &[
            "add",
            &reg_path,
            "/v",
            "DisplayName",
            "/t",
            "REG_SZ",
            "/d",
            name,
            "/f",
        ],
    )?;
    let r = finish(base_result(&found), &out, &[0], "reg add");
    let mut v = serde_json::to_value(r)?;
    v["backupPath"] = json!(backup.to_string_lossy());
    v["newName"] = json!(name);
    Ok(v)
}

// ---------------------------------------------------------------- leftovers

fn validate_leftover_params(p: &LeftoverParams) -> Result<()> {
    if p.name.trim().is_empty() || p.name.len() > 300 || p.id.len() > 1000 {
        return Err(ApiError::invalid_params(
            "`name` or `id` is missing or too long",
        ));
    }
    if p.paths.len() > 5000 {
        return Err(ApiError::invalid_params("too many paths"));
    }
    Ok(())
}

fn leftovers_handler(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: LeftoverParams = serde_json::from_value(params)?;
    validate_leftover_params(&p)?;
    let v = leftovers::scan(ctx, &p.name, &p.id, p.bundle_id.as_deref());
    Ok(serde_json::to_value(v)?)
}

fn normalize_name(n: &str) -> String {
    n.split(['(', '['])
        .next()
        .unwrap_or(n)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn remove_leftovers_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: LeftoverParams = serde_json::from_value(params)?;
    validate_leftover_params(&p)?;
    if p.paths.is_empty() {
        return Err(ApiError::invalid_params("`paths` is empty"));
    }
    // Only applications that ClearSweep itself uninstalled: the leftover cleaner must not be
    // usable to point at the data of a program that is in use.
    let recorded = ledger::find(ctx, &p.id, &p.name).ok_or_else(|| {
        ApiError::permission_denied(format!(
            "`{}` was not uninstalled by ClearSweep (recently), so its leftovers cannot be removed from here",
            p.name
        ))
    })?;
    // Never touch data of something that is still installed (same id or same name).
    let want = normalize_name(&p.name);
    if collect_with(ctx, job, false)?
        .iter()
        .any(|f| f.entry.id == p.id || normalize_name(&f.entry.name) == want)
    {
        return Err(ApiError::invalid_params(format!(
            "`{}` is still installed; uninstall it before removing its leftovers",
            p.name
        )));
    }
    let allowed = leftovers::scan(ctx, &p.name, &p.id, recorded.bundle_id.as_deref());
    let results = leftovers::remove_verified(ctx, &p.paths, &allowed, job)?;
    let freed: u64 = results.iter().map(|r| r.bytes).sum();
    Ok(json!({ "results": results, "totalBytes": freed }))
}
