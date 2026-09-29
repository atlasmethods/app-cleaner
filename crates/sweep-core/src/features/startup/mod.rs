//! Startup manager: list, enable, disable and remove startup items.
//!
//! Methods:
//! - `startup.list`: every startup item of this system (autostart entries, services,
//!   scheduled tasks, context-menu handlers, cron `@reboot` lines, launch agents, login items).
//! - `startup.set_enabled { id, enabled }`: switch an item on or off. Items are looked up
//!   again on the server: the client's id only *selects*, it never supplies a command or a
//!   path. Critical items are refused.
//! - `startup.remove { id }`: back the item up to `<data>/backups/startup-<ts>/` and delete
//!   it; nothing is deleted when the backup cannot be written.
//! - `startup.restore_backup { id }`: put a removed item back.
//!
//! Platform logic lives in [`linux`], [`windows`] and [`macos`]; the OS is taken from
//! `ctx.env.os`, so every platform's parsing and commands are unit-tested on any host.

use serde::Deserialize;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::api::Registry;
use crate::ctx::{Ctx, Os};
use crate::elevate::run_privileged;
use crate::error::{ApiError, Result};
use crate::job::{Job, ProgressEvent};
use crate::pkgutil::summarize;

pub mod backup;
pub mod impact;
pub mod linux;
pub mod macos;
pub mod model;
pub mod windows;

#[cfg(test)]
mod tests;

use backup::{read_manifest, Backup, Committed};
pub use model::{Entry, Impact, Kind, Scope, StartupItem, Target, ToggleOpts};
use model::WinHive;
use windows::{RegData, StartupRegistry};

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "startup.list",
    "startup.set_enabled",
    "startup.remove",
    "startup.restore_backup",
];

pub fn register(r: &mut Registry) {
    r.add("startup.list", list_handler);
    r.add("startup.set_enabled", set_enabled_handler);
    r.add("startup.remove", remove_handler);
    r.add("startup.restore_backup", restore_handler);
}

// ---------------------------------------------------------------- registry access

type RegHandle = Rc<dyn StartupRegistry>;

thread_local! {
    static REG_OVERRIDE: RefCell<Option<RegHandle>> = const { RefCell::new(None) };
}

/// Run `f` with `reg` standing in for the Windows registry on this thread (tests only).
#[cfg(test)]
pub fn with_registry<T>(reg: impl StartupRegistry + 'static, f: impl FnOnce() -> T) -> T {
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

/// A registry with nothing in it: used where the real Windows registry does not exist.
#[cfg_attr(windows, allow(dead_code))]
struct NoRegistry;

impl StartupRegistry for NoRegistry {
    fn values(&self, _: WinHive, _: &str) -> Vec<(String, RegData)> {
        Vec::new()
    }
    fn subkeys(&self, _: WinHive, _: &str) -> Vec<String> {
        Vec::new()
    }
    fn default_value(&self, _: WinHive, _: &str) -> Option<String> {
        None
    }
    fn key_exists(&self, _: WinHive, _: &str) -> bool {
        false
    }
}

fn registry_handle() -> RegHandle {
    if let Some(r) = REG_OVERRIDE.with(|r| r.borrow().clone()) {
        return r;
    }
    #[cfg(windows)]
    {
        Rc::new(windows::RealStartupRegistry)
    }
    #[cfg(not(windows))]
    {
        Rc::new(NoRegistry)
    }
}

// ---------------------------------------------------------------- collection

/// Everything on this system, sorted by kind then name, with impact filled in from the
/// running processes.
pub fn collect(ctx: &Ctx, job: &Job) -> Result<Vec<Entry>> {
    let procs = ctx.procs.details();
    collect_with_procs(ctx, job, &procs)
}

/// [`collect`] with a process snapshot the caller already has.
pub fn collect_with_procs(
    ctx: &Ctx,
    job: &Job,
    procs: &[crate::procs::ProcDetail],
) -> Result<Vec<Entry>> {
    let step = |m: &str| job.progress(ProgressEvent::new("list").message(m.to_string()));
    let mut all: Vec<Entry> = Vec::new();
    match ctx.env.os {
        Os::Linux => {
            step("Reading autostart entries");
            all.extend(linux::collect_xdg(ctx));
            job.check_cancelled()?;
            step("Reading services");
            all.extend(linux::collect_systemd(ctx));
            job.check_cancelled()?;
            step("Reading cron jobs");
            all.extend(linux::collect_cron(ctx));
        }
        Os::Windows => {
            let reg = registry_handle();
            step("Reading startup entries");
            all.extend(windows::collect_run(&*reg));
            all.extend(windows::collect_folders(ctx, &*reg));
            job.check_cancelled()?;
            step("Reading scheduled tasks");
            all.extend(windows::collect_tasks(ctx));
            job.check_cancelled()?;
            step("Reading services");
            all.extend(windows::collect_services(ctx));
            job.check_cancelled()?;
            step("Reading context menu handlers");
            all.extend(windows::collect_context(ctx, &*reg));
        }
        Os::MacOs => {
            step("Reading launch agents");
            all.extend(macos::collect_launchd(ctx));
            job.check_cancelled()?;
            step("Reading login items");
            all.extend(macos::collect_login_items(ctx));
        }
    }
    // Ids are the only handle a client gets, so they must be unique.
    let mut seen = std::collections::HashSet::new();
    all.retain(|e| seen.insert(e.item.id.clone()));

    step("Measuring impact");
    for e in &mut all {
        e.item.impact = impact::impact_for(e.exe.as_deref(), procs);
    }
    all.sort_by(|a, b| {
        (kind_order(a.item.kind), a.item.name.to_lowercase())
            .cmp(&(kind_order(b.item.kind), b.item.name.to_lowercase()))
    });
    Ok(all)
}

fn kind_order(k: Kind) -> u8 {
    match k {
        Kind::Autostart => 0,
        Kind::LoginItem => 1,
        Kind::LaunchAgent => 2,
        Kind::LaunchDaemon => 3,
        Kind::Service => 4,
        Kind::ScheduledTask => 5,
        Kind::Cron => 6,
        Kind::ContextMenu => 7,
    }
}

/// Does changing this item need administrator rights?
pub fn needs_admin(t: &Target) -> bool {
    match t {
        Target::Systemd { user, .. } => !user,
        Target::WinRun { loc, .. } => loc.hive.needs_admin(),
        Target::WinFolder { all_users, .. } => *all_users,
        Target::WinService { .. } => true,
        Target::WinContext { hive, .. } => hive.needs_admin(),
        Target::MacPlist { daemon, .. } => *daemon,
        _ => false,
    }
}

// ---------------------------------------------------------------- params

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct SetParams {
    id: String,
    enabled: bool,
}

fn find<'a>(entries: &'a [Entry], id: &str) -> Result<&'a Entry> {
    entries
        .iter()
        .find(|e| e.item.id == id)
        .ok_or_else(|| ApiError::not_found("that startup item no longer exists"))
}

// ---------------------------------------------------------------- set_enabled

/// Switch `e` on or off. Refuses critical items.
pub fn apply_enabled(ctx: &Ctx, e: &Entry, enabled: bool, opts: ToggleOpts) -> Result<()> {
    if e.item.critical {
        return Err(ApiError::permission_denied(format!(
            "`{}` is needed by the system and cannot be changed",
            e.item.name
        )));
    }
    if !e.item.can_disable {
        return Err(ApiError::unsupported(format!(
            "`{}` cannot be switched off and on; delete it instead",
            e.item.name
        )));
    }
    if e.item.enabled == enabled {
        return Ok(());
    }
    match &e.target {
        Target::Xdg {
            user_path,
            system_path,
            ..
        } => linux::xdg_set_enabled(ctx, user_path, system_path.as_deref(), enabled),
        Target::Systemd { unit, user } => linux::systemd_set_enabled(ctx, unit, *user, enabled),
        Target::Cron { line, nth } => {
            let text = linux::read_crontab(ctx)
                .ok_or_else(|| ApiError::not_found("could not read the crontab"))?;
            let new = linux::cron_toggle(&text, line, *nth, enabled)?;
            linux::install_crontab(ctx, &new)
        }
        Target::WinContext { .. } => {
            // A backup of the handlers key first, as for every registry edit.
            let mut b = Backup::create(
                ctx,
                "startup",
                &format!("Before switching {} {}", e.item.name, on_off(enabled)),
            )?;
            if let Err(err) = stage_backup(ctx, e, &mut b) {
                b.abort();
                return Err(err);
            }
            b.commit()?;
            windows::set_enabled(ctx, &e.target, enabled)
        }
        Target::WinRun { .. }
        | Target::WinFolder { .. }
        | Target::WinTask { .. }
        | Target::WinService { .. } => windows::set_enabled(ctx, &e.target, enabled),
        Target::MacPlist { .. } => macos::set_enabled(ctx, &e.target, enabled, opts),
        Target::MacLogin { .. } => Err(ApiError::unsupported(
            "Login items cannot be switched off; delete the item instead",
        )),
    }
}

fn on_off(enabled: bool) -> &'static str {
    if enabled {
        "on"
    } else {
        "off"
    }
}

/// Look `id` up again, change it, and return the item as it is afterwards.
pub fn set_enabled_by_id(
    ctx: &Ctx,
    job: &Job,
    id: &str,
    enabled: bool,
    opts: ToggleOpts,
) -> Result<StartupItem> {
    let entries = collect(ctx, job)?;
    let e = find(&entries, id)?;
    apply_enabled(ctx, e, enabled, opts)?;
    let after = collect(ctx, job)?;
    Ok(find(&after, id)
        .map(|e| e.item.clone())
        .unwrap_or_else(|_| e.item.clone()))
}

fn list_handler(ctx: &Ctx, _params: Value, job: &Job) -> Result<Value> {
    let entries = collect(ctx, job)?;
    let items: Vec<&StartupItem> = entries.iter().map(|e| &e.item).collect();
    Ok(serde_json::to_value(items)?)
}

fn set_enabled_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: SetParams = serde_json::from_value(params)?;
    // A macOS agent that is switched off from the UI is also stopped for this session.
    let item = set_enabled_by_id(ctx, job, &p.id, p.enabled, ToggleOpts { stop_now: true })?;
    Ok(json!({ "ok": true, "item": item }))
}

// ---------------------------------------------------------------- backup + remove

fn reg_export(ctx: &Ctx, b: &mut Backup, hive: WinHive, key: &str) -> Result<()> {
    let full = format!(r"{}\{}", hive.name(), key);
    let file = b.stage_path("key.reg");
    let file_s = file.to_string_lossy().into_owned();
    let out = ctx.runner.run("reg", &["export", &full, &file_s, "/y"])?;
    if !out.success() || !file.exists() {
        return Err(ApiError::io(format!(
            "could not back up {full} first: {}",
            summarize(&out)
        )));
    }
    let rel = b.rel(&file);
    b.add_item(json!({
        "type": "regfile",
        "key": full,
        "backup": rel,
        "admin": hive.needs_admin(),
    }));
    Ok(())
}

/// Save everything needed to put `e` back. Any failure aborts the operation.
fn stage_backup(ctx: &Ctx, e: &Entry, b: &mut Backup) -> Result<()> {
    match &e.target {
        Target::Xdg { user_path, .. } => {
            let copy = b.copy_file(user_path)?;
            let rel = b.rel(&copy);
            b.add_item(json!({
                "type": "file",
                "original": user_path.to_string_lossy(),
                "backup": rel,
            }));
        }
        Target::WinFolder { path, .. } | Target::MacPlist { path, .. } => {
            let copy = b.copy_file(path)?;
            let rel = b.rel(&copy);
            b.add_item(json!({
                "type": "file",
                "original": path.to_string_lossy(),
                "backup": rel,
            }));
        }
        Target::Cron { line, .. } => {
            let text = linux::read_crontab(ctx)
                .ok_or_else(|| ApiError::io("could not read the crontab to back it up"))?;
            let file = b.write_text("crontab.txt", &text)?;
            let rel = b.rel(&file);
            b.add_item(json!({ "type": "cronline", "line": line, "backup": rel }));
        }
        Target::WinRun { loc, .. } => {
            reg_export(ctx, b, loc.hive, &loc.key)?;
            if let Some(sub) = loc.approved {
                let ak = windows::approved_key(loc.hive, sub);
                if registry_handle().key_exists(loc.hive, &ak) {
                    reg_export(ctx, b, loc.hive, &ak)?;
                }
            }
        }
        Target::WinContext { hive, key, .. } => reg_export(ctx, b, *hive, key)?,
        Target::WinTask { name } => {
            if !windows::arg_ok(name) {
                return Err(ApiError::invalid_params("unsupported task name"));
            }
            let out = ctx
                .runner
                .run("schtasks", &["/query", "/tn", name, "/xml"])?;
            if !out.success() || !out.stdout.contains("<Task") {
                return Err(ApiError::io(format!(
                    "could not export the task first: {}",
                    summarize(&out)
                )));
            }
            let file = b.write_text("task.xml", &out.stdout)?;
            let rel = b.rel(&file);
            b.add_item(json!({ "type": "schtask", "name": name, "backup": rel }));
        }
        Target::MacLogin { name, path } => {
            b.add_item(json!({ "type": "loginitem", "name": name, "path": path }));
        }
        Target::Systemd { .. } | Target::WinService { .. } => {
            return Err(ApiError::unsupported("Services cannot be deleted"));
        }
    }
    Ok(())
}

/// Back `e` up and delete it. The backup is committed (manifest written) before anything is
/// deleted, and a failing backup stops everything.
pub fn remove_entry(ctx: &Ctx, e: &Entry) -> Result<Committed> {
    if e.item.critical {
        return Err(ApiError::permission_denied(format!(
            "`{}` is needed by the system and cannot be removed",
            e.item.name
        )));
    }
    if !e.item.can_delete {
        return Err(ApiError::unsupported(format!(
            "`{}` cannot be deleted; switch it off instead",
            e.item.name
        )));
    }
    let mut b = Backup::create(ctx, "startup", &format!("Removed startup item {}", e.item.name))?;
    if let Err(err) = stage_backup(ctx, e, &mut b) {
        b.abort();
        return Err(err);
    }
    let done = b.commit()?;
    let res = delete_target(ctx, e);
    if let Err(err) = res {
        return Err(ApiError::new(
            err.code,
            format!(
                "{} (a backup was saved as {})",
                err.message, done.id
            ),
        ));
    }
    Ok(done)
}

fn delete_target(ctx: &Ctx, e: &Entry) -> Result<()> {
    match &e.target {
        Target::Xdg { user_path, .. } => {
            linux::safe_remove_file(ctx, &linux::user_autostart_dir(ctx), user_path)
        }
        Target::Cron { line, nth } => {
            let text = linux::read_crontab(ctx)
                .ok_or_else(|| ApiError::not_found("could not read the crontab"))?;
            linux::install_crontab(ctx, &linux::cron_remove(&text, line, *nth)?)
        }
        Target::WinRun { loc, name } => windows::delete_run_value(ctx, loc, name),
        Target::WinFolder { path, .. } => {
            let dir = path
                .parent()
                .ok_or_else(|| ApiError::internal("startup file has no folder"))?;
            linux::safe_remove_file(ctx, dir, path)
        }
        Target::WinTask { name } => windows::delete_task(ctx, name),
        Target::WinContext { hive, key, handler } => {
            windows::delete_context_handler(ctx, *hive, key, handler)
        }
        Target::MacPlist {
            path,
            label,
            domain,
            in_home,
            ..
        } => {
            if !*in_home {
                return Err(ApiError::unsupported(
                    "Machine-wide launch items cannot be deleted; switch them off instead",
                ));
            }
            // Unload first so the job does not keep running from a file that is gone.
            let _ = ctx
                .runner
                .run("launchctl", &["bootout", &format!("{domain}/{label}")]);
            let dir = path
                .parent()
                .ok_or_else(|| ApiError::internal("launch agent has no folder"))?;
            linux::safe_remove_file(ctx, dir, path)
        }
        Target::MacLogin { name, .. } => macos::delete_login_item(ctx, name),
        Target::Systemd { .. } | Target::WinService { .. } => {
            Err(ApiError::unsupported("Services cannot be deleted"))
        }
    }
}

fn remove_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: IdParams = serde_json::from_value(params)?;
    let entries = collect(ctx, job)?;
    let e = find(&entries, &p.id)?;
    let done = remove_entry(ctx, e)?;
    Ok(json!({
        "ok": true,
        "id": p.id,
        "backupId": done.id,
        "backupPath": done.dir.to_string_lossy(),
    }))
}

// ---------------------------------------------------------------- restore

/// Directories a restored file may be written into: the places startup files live.
fn restore_dirs(ctx: &Ctx) -> Vec<PathBuf> {
    let mut v = vec![
        linux::user_autostart_dir(ctx),
        ctx.env.home.join("Library/LaunchAgents"),
    ];
    for (_, d) in windows::startup_folders(ctx) {
        v.push(d);
    }
    v
}

fn restore_target_ok(ctx: &Ctx, original: &Path) -> bool {
    let Some(parent) = original.parent() else {
        return false;
    };
    original.file_name().is_some()
        && restore_dirs(ctx).iter().any(|d| {
            crate::safety::normalize(d) == crate::safety::normalize(parent)
        })
}

fn restore_handler(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: IdParams = serde_json::from_value(params)?;
    let (dir, manifest) = read_manifest(ctx, "startup", &p.id)?;
    let items = manifest
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut restored = 0usize;
    let mut notes: Vec<String> = Vec::new();
    for it in &items {
        let ty = it.get("type").and_then(Value::as_str).unwrap_or("");
        let backup_file = it
            .get("backup")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|r| !r.contains("..") && !r.starts_with('/') && !r.contains('\\'))
            .map(|r| dir.join(r));
        match ty {
            "file" => {
                let original = it
                    .get("original")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .ok_or_else(|| ApiError::io("damaged backup: no original path"))?;
                let src = backup_file.ok_or_else(|| ApiError::io("damaged backup: no file"))?;
                if !restore_target_ok(ctx, &original) {
                    return Err(ApiError::permission_denied(format!(
                        "refusing to restore into {}",
                        original.display()
                    )));
                }
                if original.exists() {
                    notes.push(format!("{} already exists; left as it is", original.display()));
                    continue;
                }
                if let Some(parent) = original.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let bytes = std::fs::read(&src)
                    .map_err(|e| ApiError::io(format!("could not read the backup: {e}")))?;
                crate::fsutil::atomic_write(&original, &bytes)?;
                restored += 1;
            }
            "regfile" => {
                let src = backup_file.ok_or_else(|| ApiError::io("damaged backup: no file"))?;
                let admin = it.get("admin").and_then(Value::as_bool).unwrap_or(false);
                let s = src.to_string_lossy().into_owned();
                let out = if admin {
                    run_privileged(ctx, "reg", &["import", &s])?
                } else {
                    ctx.runner.run("reg", &["import", &s])?
                };
                if !out.success() {
                    return Err(ApiError::io(format!("reg import failed: {}", summarize(&out))));
                }
                restored += 1;
            }
            "schtask" => {
                let src = backup_file.ok_or_else(|| ApiError::io("damaged backup: no file"))?;
                let name = it.get("name").and_then(Value::as_str).unwrap_or("");
                if !windows::arg_ok(name) || !name.starts_with('\\') {
                    return Err(ApiError::io("damaged backup: bad task name"));
                }
                let s = src.to_string_lossy().into_owned();
                let out = ctx
                    .runner
                    .run("schtasks", &["/create", "/tn", name, "/xml", &s, "/f"])?;
                if !out.success() {
                    return Err(ApiError::io(format!(
                        "schtasks /create failed: {}",
                        summarize(&out)
                    )));
                }
                restored += 1;
            }
            "cronline" => {
                let line = it.get("line").and_then(Value::as_str).unwrap_or("");
                if linux::cron_reboot_line(line).is_none() {
                    return Err(ApiError::io("damaged backup: not a cron @reboot line"));
                }
                let mut text = linux::read_crontab(ctx).unwrap_or_default();
                let present = text
                    .lines()
                    .any(|l| linux::cron_reboot_line(l).is_some_and(|(_, b)| b == line));
                if present {
                    notes.push("the cron line is already there".into());
                    continue;
                }
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(line);
                text.push('\n');
                linux::install_crontab(ctx, &text)?;
                restored += 1;
            }
            "loginitem" => {
                let path = it.get("path").and_then(Value::as_str).unwrap_or("");
                if path.is_empty() {
                    return Err(ApiError::io("damaged backup: no login item path"));
                }
                let out = ctx
                    .runner
                    .run("osascript", &["-e", &macos::make_login_item_script(path)])?;
                if !out.success() {
                    return Err(ApiError::io(format!(
                        "could not re-create the login item: {}",
                        summarize(&out)
                    )));
                }
                restored += 1;
            }
            other => notes.push(format!("unknown backup item type `{other}` skipped")),
        }
    }
    Ok(json!({ "ok": true, "id": p.id, "restored": restored, "notes": notes }))
}

