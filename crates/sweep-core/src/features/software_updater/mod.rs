//! Software updater: detect outdated applications and update them through the platform's
//! package manager.
//!
//! Methods:
//! - `software_updater.list { refresh? }`: available updates from every package manager found
//!   (`apt`, `dnf`, `pacman`, `flatpak`, `snap`, `winget`, `brew`, `softwareupdate`).
//!   `refresh` first updates the package index (`apt-get update`, `dnf makecache`, `brew update`).
//! - `software_updater.update { ids }`, `software_updater.update_all`: run the updates. The ids
//!   are matched against a fresh server-side listing, so a client can only update packages that
//!   really have an update available; ignored ids are skipped by `update_all`.
//! - `software_updater.set_ignored { id, ignored }`: persisted in settings (`ignoredUpdates`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use crate::api::Registry;
use crate::ctx::{Ctx, Os};
use crate::elevate::run_privileged;
use crate::error::{ApiError, ErrorCode, Result};
use crate::features::settings;
use crate::job::{Job, ProgressEvent};
use crate::pkgutil::{summarize, valid_pkg_name};
use crate::runner::CmdOutput;

pub mod parse;
#[cfg(test)]
mod tests;

use parse::{Item, USource};

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "software_updater.list",
    "software_updater.update",
    "software_updater.update_all",
    "software_updater.set_ignored",
];

pub fn register(r: &mut Registry) {
    r.add("software_updater.list", list_handler);
    r.add("software_updater.update", update_handler);
    r.add("software_updater.update_all", update_all_handler);
    r.add("software_updater.set_ignored", set_ignored_handler);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEntry {
    pub id: String,
    pub name: String,
    pub current_version: String,
    pub new_version: String,
    pub source: USource,
    pub ignored: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateResult {
    pub id: String,
    pub name: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub message: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ListParams {
    refresh: bool,
}

#[derive(Deserialize)]
struct UpdateParams {
    ids: Vec<String>,
}

#[derive(Deserialize)]
struct IgnoreParams {
    id: String,
    ignored: bool,
}

// ---------------------------------------------------------------- collection

fn has(ctx: &Ctx, p: &str) -> bool {
    ctx.runner.which(p).is_some()
}

/// Outcome of asking one package manager.
struct Probe {
    items: Vec<Item>,
    /// Set when the tool ran but failed.
    error: Option<String>,
}

fn ok_probe(items: Vec<Item>) -> Probe {
    Probe { items, error: None }
}

fn fail_probe(what: &str, out: &CmdOutput) -> Probe {
    Probe {
        items: Vec::new(),
        error: Some(format!(
            "{what} failed (exit code {}): {}",
            out.status,
            summarize(out)
        )),
    }
}

fn err_probe(what: &str, e: ApiError) -> Probe {
    Probe {
        items: Vec::new(),
        error: Some(format!("{what}: {}", e.message)),
    }
}

/// `apt-get update`, `dnf makecache`, `brew update`. Cancelling the authorization dialog
/// aborts the listing; other failures leave the (possibly stale) index in place.
fn refresh_indexes(ctx: &Ctx, job: &Job) -> Result<()> {
    job.progress(ProgressEvent::new("refresh").message("Refreshing package lists".to_string()));
    let cancelled = |e: ApiError| -> Result<()> {
        if e.code == ErrorCode::PermissionDenied {
            Err(e)
        } else {
            Ok(())
        }
    };
    match ctx.env.os {
        Os::Linux => {
            if has(ctx, "apt") || has(ctx, "apt-get") {
                if let Err(e) = run_privileged(ctx, "apt-get", &["update"]) {
                    cancelled(e)?;
                }
            }
            job.check_cancelled()?;
            if has(ctx, "dnf") {
                if let Err(e) = run_privileged(ctx, "dnf", &["makecache"]) {
                    cancelled(e)?;
                }
            }
            job.check_cancelled()?;
            if has(ctx, "brew") {
                let _ = ctx.runner.run("brew", &["update"]);
            }
        }
        Os::MacOs => {
            if has(ctx, "brew") {
                let _ = ctx.runner.run("brew", &["update"]);
            }
        }
        Os::Windows => {}
    }
    Ok(())
}

fn probe_apt(ctx: &Ctx) -> Probe {
    match ctx.runner.run("apt", &["list", "--upgradable"]) {
        Ok(o) if o.success() => ok_probe(parse::parse_apt(&o.stdout)),
        Ok(o) => fail_probe("apt list", &o),
        Err(e) => err_probe("apt list", e),
    }
}

fn probe_dnf(ctx: &Ctx) -> Probe {
    // Exit status 100 = updates are available; 0 = none; anything else is an error.
    match ctx.runner.run("dnf", &["check-update"]) {
        Ok(o) if o.status == 0 || o.status == 100 => {
            let mut items = if o.status == 100 {
                parse::parse_dnf(&o.stdout)
            } else {
                Vec::new()
            };
            if !items.is_empty() && has(ctx, "rpm") {
                let mut args: Vec<String> = vec![
                    "-q".into(),
                    "--queryformat".into(),
                    "%{NAME}.%{ARCH}\\t%{VERSION}-%{RELEASE}\\n".into(),
                ];
                args.extend(items.iter().map(|i| i.name.clone()));
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                if let Ok(q) = ctx.runner.run("rpm", &refs) {
                    let cur = parse::parse_rpm_versions(&q.stdout);
                    for it in &mut items {
                        if let Some(v) = cur.get(&it.key) {
                            it.current = v.clone();
                        }
                    }
                }
            }
            ok_probe(items)
        }
        Ok(o) => fail_probe("dnf check-update", &o),
        Err(e) => err_probe("dnf check-update", e),
    }
}

fn probe_pacman(ctx: &Ctx) -> Probe {
    // `checkupdates` (pacman-contrib) works on a private copy of the sync db and exits 2 when
    // there is nothing to do; `pacman -Qu` exits 1 in that case.
    if has(ctx, "checkupdates") {
        match ctx.runner.run("checkupdates", &[]) {
            Ok(o) if o.status == 0 || o.status == 2 => ok_probe(parse::parse_pacman(&o.stdout)),
            Ok(o) => fail_probe("checkupdates", &o),
            Err(e) => err_probe("checkupdates", e),
        }
    } else {
        match ctx.runner.run("pacman", &["-Qu"]) {
            Ok(o) if o.status == 0 || (o.status == 1 && o.stdout.trim().is_empty()) => {
                ok_probe(parse::parse_pacman(&o.stdout))
            }
            Ok(o) => fail_probe("pacman -Qu", &o),
            Err(e) => err_probe("pacman -Qu", e),
        }
    }
}

fn probe_flatpak(ctx: &Ctx) -> Probe {
    match ctx.runner.run(
        "flatpak",
        &[
            "remote-ls",
            "--updates",
            "--app",
            "--columns=application,version",
        ],
    ) {
        Ok(o) if o.success() => {
            let mut items = parse::parse_flatpak_updates(&o.stdout);
            if !items.is_empty() {
                if let Ok(l) = ctx.runner.run(
                    "flatpak",
                    &["list", "--app", "--columns=application,name,version"],
                ) {
                    let inst = parse::parse_flatpak_installed(&l.stdout);
                    for it in &mut items {
                        if let Some((name, ver)) = inst.get(&it.key) {
                            if !name.is_empty() {
                                it.name = name.clone();
                            }
                            it.current = ver.clone();
                        }
                    }
                }
            }
            ok_probe(items)
        }
        Ok(o) => fail_probe("flatpak remote-ls", &o),
        Err(e) => err_probe("flatpak remote-ls", e),
    }
}

fn probe_snap(ctx: &Ctx) -> Probe {
    match ctx.runner.run("snap", &["refresh", "--list"]) {
        Ok(o) if o.success() => {
            let mut items = parse::parse_snap_refresh(&o.stdout);
            if !items.is_empty() {
                if let Ok(l) = ctx.runner.run("snap", &["list"]) {
                    let cur: HashMap<String, String> =
                        crate::features::uninstall::linux::parse_snap(&l.stdout)
                            .into_iter()
                            .map(|f| (f.entry.name, f.entry.version))
                            .collect();
                    for it in &mut items {
                        if let Some(v) = cur.get(&it.key) {
                            it.current = v.clone();
                        }
                    }
                }
            }
            ok_probe(items)
        }
        Ok(o) => fail_probe("snap refresh --list", &o),
        Err(e) => err_probe("snap refresh --list", e),
    }
}

fn probe_winget(ctx: &Ctx) -> Probe {
    match ctx.runner.run("winget", &["upgrade", "--include-unknown"]) {
        // winget exits non-zero (0x8A150014 "no applicable upgrade") when nothing is available;
        // the table parser simply finds no rows in that case.
        Ok(o) => {
            let items = parse::parse_winget(&o.stdout);
            if items.is_empty()
                && !o.success()
                && !o.stdout.trim().is_empty()
                && o.stderr.trim().is_empty()
            {
                // Informational output ("No installed package found matching input criteria.").
                return ok_probe(items);
            }
            if items.is_empty() && !o.success() && !o.stderr.trim().is_empty() {
                return fail_probe("winget upgrade", &o);
            }
            ok_probe(items)
        }
        Err(e) => err_probe("winget upgrade", e),
    }
}

fn probe_brew(ctx: &Ctx) -> Probe {
    match ctx.runner.run("brew", &["outdated", "--json=v2"]) {
        Ok(o) if o.success() => match parse::parse_brew_outdated(&o.stdout) {
            Ok(items) => ok_probe(items),
            Err(e) => Probe {
                items: Vec::new(),
                error: Some(e),
            },
        },
        Ok(o) => fail_probe("brew outdated", &o),
        Err(e) => err_probe("brew outdated", e),
    }
}

fn probe_macos(ctx: &Ctx) -> Probe {
    match ctx.runner.run("softwareupdate", &["-l"]) {
        // The tool writes its report to stderr on recent macOS versions.
        Ok(o) if o.success() => {
            let text = format!("{}\n{}", o.stdout, o.stderr);
            ok_probe(parse::macos_items(&parse::parse_softwareupdate(&text)))
        }
        Ok(o) => fail_probe("softwareupdate -l", &o),
        Err(e) => err_probe("softwareupdate -l", e),
    }
}

/// All available updates. Fails only when nothing could be queried at all.
pub fn collect(ctx: &Ctx, job: &Job, refresh: bool) -> Result<Vec<Item>> {
    if refresh {
        refresh_indexes(ctx, job)?;
    }
    let mut probes: Vec<(&str, Probe)> = Vec::new();
    let mut tools = 0usize;
    let mut step = |name: &'static str, f: &dyn Fn(&Ctx) -> Probe| -> Result<()> {
        job.check_cancelled()?;
        job.progress(ProgressEvent::new("check").message(format!("Checking {name}")));
        tools += 1;
        probes.push((name, f(ctx)));
        Ok(())
    };
    match ctx.env.os {
        Os::Linux => {
            if has(ctx, "apt") {
                step("apt", &probe_apt)?;
            }
            if has(ctx, "dnf") {
                step("dnf", &probe_dnf)?;
            }
            if has(ctx, "pacman") || has(ctx, "checkupdates") {
                step("pacman", &probe_pacman)?;
            }
            if has(ctx, "flatpak") {
                step("flatpak", &probe_flatpak)?;
            }
            if has(ctx, "snap") {
                step("snap", &probe_snap)?;
            }
            if has(ctx, "brew") {
                step("brew", &probe_brew)?;
            }
        }
        Os::Windows => {
            if has(ctx, "winget") {
                step("winget", &probe_winget)?;
            }
        }
        Os::MacOs => {
            if has(ctx, "brew") {
                step("brew", &probe_brew)?;
            }
            if has(ctx, "softwareupdate") {
                step("softwareupdate", &probe_macos)?;
            }
        }
    }
    if tools == 0 {
        return Err(ApiError::unsupported(
            "No supported package manager was found (looked for apt, dnf, pacman, flatpak, snap, winget, brew and softwareupdate)",
        ));
    }
    let failed: Vec<&(&str, Probe)> = probes.iter().filter(|(_, p)| p.error.is_some()).collect();
    if failed.len() == probes.len() {
        let msg = failed
            .iter()
            .filter_map(|(_, p)| p.error.clone())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ApiError::io(msg));
    }
    let mut items: Vec<Item> = probes.into_iter().flat_map(|(_, p)| p.items).collect();
    items.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id().cmp(&b.id()))
    });
    Ok(items)
}

fn to_entries(items: Vec<Item>, ignored: &HashSet<String>) -> Vec<UpdateEntry> {
    items
        .into_iter()
        .map(|i| {
            let id = i.id();
            UpdateEntry {
                ignored: ignored.contains(&id),
                id,
                name: i.name,
                current_version: i.current,
                new_version: i.new,
                source: i.source,
                security: i.security,
            }
        })
        .collect()
}

fn ignored_set(ctx: &Ctx) -> HashSet<String> {
    settings::load(ctx).ignored_updates.into_iter().collect()
}

fn list_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: ListParams = if params.is_null() {
        ListParams::default()
    } else {
        serde_json::from_value(params)?
    };
    let items = collect(ctx, job, p.refresh)?;
    Ok(serde_json::to_value(to_entries(items, &ignored_set(ctx)))?)
}

// ---------------------------------------------------------------- updating

/// One package-manager invocation covering one or more items.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub source: USource,
    pub keys: Vec<String>,
    pub program: &'static str,
    pub args: Vec<String>,
    pub elevated: bool,
    /// Human label for progress and messages.
    pub label: String,
}

fn label_ok(l: &str) -> bool {
    !l.is_empty() && l.len() <= 200 && !l.starts_with('-') && !l.chars().any(char::is_control)
}

/// Group `items` into the commands that update them. Items with unusable ids are returned
/// separately with the reason.
pub fn plan(
    items: &[Item],
    available: &[Item],
    ignored: &HashSet<String>,
) -> (Vec<Step>, Vec<(Item, String)>) {
    let mut steps: Vec<Step> = Vec::new();
    let mut rejected: Vec<(Item, String)> = Vec::new();
    let batch = |steps: &mut Vec<Step>,
                 source: USource,
                 item: &Item,
                 program: &'static str,
                 base: &[&str],
                 elevated: bool,
                 label: &str| {
        if let Some(s) = steps
            .iter_mut()
            .find(|s| s.source == source && s.program == program)
        {
            s.keys.push(item.key.clone());
            s.args.push(item.key.clone());
        } else {
            let mut args: Vec<String> = base.iter().map(|s| s.to_string()).collect();
            args.push(item.key.clone());
            steps.push(Step {
                source,
                keys: vec![item.key.clone()],
                program,
                args,
                elevated,
                label: label.to_string(),
            });
        }
    };
    let mut pacman_done = false;
    for it in items {
        if it.source != USource::Macos && it.source != USource::Winget && !valid_pkg_name(&it.key) {
            rejected.push((it.clone(), "unsafe package name".into()));
            continue;
        }
        match it.source {
            USource::Apt => batch(
                &mut steps,
                USource::Apt,
                it,
                "apt-get",
                &["install", "--only-upgrade", "-y"],
                true,
                "apt-get",
            ),
            USource::Dnf => batch(
                &mut steps,
                USource::Dnf,
                it,
                "dnf",
                &["upgrade", "-y"],
                true,
                "dnf",
            ),
            USource::Snap => batch(
                &mut steps,
                USource::Snap,
                it,
                "snap",
                &["refresh"],
                true,
                "snap refresh",
            ),
            USource::Flatpak => batch(
                &mut steps,
                USource::Flatpak,
                it,
                "flatpak",
                &["update", "-y"],
                false,
                "flatpak update",
            ),
            USource::Pacman => {
                // Arch does not support partial upgrades: a full `-Syu`, minus ignored packages.
                if !pacman_done {
                    pacman_done = true;
                    let skip: Vec<String> = available
                        .iter()
                        .filter(|i| i.source == USource::Pacman && ignored.contains(&i.id()))
                        .map(|i| i.key.clone())
                        .collect();
                    let mut args: Vec<String> = vec!["-Syu".into(), "--noconfirm".into()];
                    if !skip.is_empty() {
                        args.push("--ignore".into());
                        args.push(skip.join(","));
                    }
                    steps.push(Step {
                        source: USource::Pacman,
                        keys: Vec::new(),
                        program: "pacman",
                        args,
                        elevated: true,
                        label: "pacman -Syu".into(),
                    });
                }
                if let Some(s) = steps.iter_mut().find(|s| s.source == USource::Pacman) {
                    s.keys.push(it.key.clone());
                }
            }
            USource::Winget => {
                if !valid_winget_id(&it.key) {
                    rejected.push((
                        it.clone(),
                        "the package id is incomplete (truncated by winget) or unsafe".into(),
                    ));
                    continue;
                }
                steps.push(Step {
                    source: USource::Winget,
                    keys: vec![it.key.clone()],
                    program: "winget",
                    args: [
                        "upgrade",
                        "--id",
                        &it.key,
                        "-e",
                        "--silent",
                        "--accept-package-agreements",
                        "--accept-source-agreements",
                    ]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                    elevated: false,
                    label: format!("winget upgrade {}", it.key),
                });
            }
            USource::Brew | USource::BrewCask => {
                let mut args = vec!["upgrade".to_string()];
                if it.source == USource::BrewCask {
                    args.push("--cask".into());
                }
                args.push(it.key.clone());
                steps.push(Step {
                    source: it.source,
                    keys: vec![it.key.clone()],
                    program: "brew",
                    args,
                    elevated: false,
                    label: format!("brew upgrade {}", it.key),
                });
            }
            USource::Macos => {
                if !label_ok(&it.key) {
                    rejected.push((it.clone(), "unsafe update label".into()));
                    continue;
                }
                steps.push(Step {
                    source: USource::Macos,
                    keys: vec![it.key.clone()],
                    program: "softwareupdate",
                    args: vec!["-i".into(), it.key.clone()],
                    elevated: true,
                    label: format!("softwareupdate -i {}", it.key),
                });
            }
        }
    }
    (steps, rejected)
}

fn valid_winget_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && !id.ends_with('…')
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-+".contains(c))
}

fn run_step(ctx: &Ctx, s: &Step) -> Result<CmdOutput> {
    let args: Vec<&str> = s.args.iter().map(String::as_str).collect();
    if s.elevated {
        run_privileged(ctx, s.program, &args)
    } else {
        ctx.runner.run(s.program, &args)
    }
}

/// Run the updates for `items` (already verified against the current listing).
pub fn run_updates(
    ctx: &Ctx,
    job: &Job,
    items: &[Item],
    available: &[Item],
    ignored: &HashSet<String>,
) -> Result<Vec<UpdateResult>> {
    let (steps, rejected) = plan(items, available, ignored);
    let by_id: HashMap<String, &Item> = items.iter().map(|i| (i.id(), i)).collect();
    let mut results: Vec<UpdateResult> = Vec::new();
    for (it, why) in rejected {
        results.push(UpdateResult {
            id: it.id(),
            name: it.name.clone(),
            ok: false,
            exit_code: None,
            message: format!("Skipped: {why}"),
        });
    }
    let total = items.len() as u64;
    let mut done = 0u64;
    let mut abort: Option<String> = None;
    let mk = |source: USource, key: &str| format!("{}:{}", source.prefix(), key);
    for s in &steps {
        if let Some(reason) = &abort {
            for k in &s.keys {
                let id = mk(s.source, k);
                results.push(UpdateResult {
                    name: by_id.get(&id).map(|i| i.name.clone()).unwrap_or_default(),
                    id,
                    ok: false,
                    exit_code: None,
                    message: format!("Not run: {reason}"),
                });
            }
            continue;
        }
        let first_name = s
            .keys
            .first()
            .and_then(|k| by_id.get(&mk(s.source, k)))
            .map(|i| i.name.clone())
            .unwrap_or_default();
        let msg = if s.keys.len() > 1 {
            format!("Updating {first_name} and {} more", s.keys.len() - 1)
        } else {
            format!("Updating {first_name}")
        };
        job.progress(
            ProgressEvent::new("update")
                .counts(done, total)
                .fraction(done as f64 / total.max(1) as f64)
                .message(msg),
        );
        if job.is_cancelled() {
            abort = Some("cancelled".into());
            for k in &s.keys {
                let id = mk(s.source, k);
                results.push(UpdateResult {
                    name: by_id.get(&id).map(|i| i.name.clone()).unwrap_or_default(),
                    id,
                    ok: false,
                    exit_code: None,
                    message: "Not run: cancelled".into(),
                });
            }
            continue;
        }
        let outcome = run_step(ctx, s);
        let (ok, code, message) = match &outcome {
            Ok(o) => {
                let sum = summarize(o);
                (
                    o.success(),
                    Some(o.status),
                    if o.success() {
                        match (s.source, sum.is_empty()) {
                            (USource::Pacman, _) => "Updated (Arch requires a full system upgrade, so all pending updates were installed)".to_string(),
                            (_, true) => "Updated".to_string(),
                            (_, false) => sum,
                        }
                    } else if sum.is_empty() {
                        format!("{} failed (exit code {})", s.label, o.status)
                    } else {
                        format!("{} failed (exit code {}): {sum}", s.label, o.status)
                    },
                )
            }
            Err(e) => {
                if e.code == ErrorCode::PermissionDenied {
                    abort = Some(e.message.clone());
                }
                (false, None, e.message.clone())
            }
        };
        // Pacman's full upgrade reports on every pending pacman item, not just the selected ones.
        let keys: Vec<String> = if s.source == USource::Pacman {
            available
                .iter()
                .filter(|i| i.source == USource::Pacman && !ignored.contains(&i.id()))
                .map(|i| i.key.clone())
                .collect()
        } else {
            s.keys.clone()
        };
        for k in &keys {
            let id = mk(s.source, k);
            results.push(UpdateResult {
                name: by_id.get(&id).map(|i| i.name.clone()).unwrap_or_default(),
                id,
                ok,
                exit_code: code,
                message: message.clone(),
            });
        }
        done += s.keys.len().max(1) as u64;
    }
    job.progress(
        ProgressEvent::new("update")
            .counts(total, total)
            .fraction(1.0),
    );
    results.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(results)
}

fn results_value(results: Vec<UpdateResult>) -> Result<Value> {
    let succeeded = results.iter().filter(|r| r.ok).count();
    let failed = results.len() - succeeded;
    Ok(serde_json::json!({ "results": results, "succeeded": succeeded, "failed": failed }))
}

fn update_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: UpdateParams = serde_json::from_value(params)?;
    if p.ids.is_empty() {
        return Err(ApiError::invalid_params("`ids` is empty"));
    }
    if p.ids.len() > 5000 {
        return Err(ApiError::invalid_params("too many ids"));
    }
    let available = collect(ctx, job, false)?;
    let ignored = ignored_set(ctx);
    let wanted: HashSet<&str> = p.ids.iter().map(String::as_str).collect();
    let chosen: Vec<Item> = available
        .iter()
        .filter(|i| wanted.contains(i.id().as_str()))
        .cloned()
        .collect();
    let known: HashSet<String> = chosen.iter().map(|i| i.id()).collect();
    let mut results: Vec<UpdateResult> = Vec::new();
    let mut seen = HashSet::new();
    for id in &p.ids {
        if !known.contains(id) && seen.insert(id.clone()) {
            results.push(UpdateResult {
                id: id.clone(),
                name: String::new(),
                ok: false,
                exit_code: None,
                message: "Refused: no update is available for this id".into(),
            });
        }
    }
    if !chosen.is_empty() {
        results.extend(run_updates(ctx, job, &chosen, &available, &ignored)?);
    }
    results_value(results)
}

fn update_all_handler(ctx: &Ctx, _params: Value, job: &Job) -> Result<Value> {
    let available = collect(ctx, job, false)?;
    let ignored = ignored_set(ctx);
    let chosen: Vec<Item> = available
        .iter()
        .filter(|i| !ignored.contains(&i.id()))
        .cloned()
        .collect();
    let results = if chosen.is_empty() {
        Vec::new()
    } else {
        run_updates(ctx, job, &chosen, &available, &ignored)?
    };
    results_value(results)
}

fn set_ignored_handler(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: IgnoreParams = serde_json::from_value(params)?;
    let id = p.id.trim().to_string();
    if id.is_empty() || id.len() > 300 || id.chars().any(char::is_control) {
        return Err(ApiError::invalid_params("`id` is missing or invalid"));
    }
    let s = settings::update(ctx, |s| {
        s.ignored_updates.retain(|x| x != &id);
        if p.ignored {
            s.ignored_updates.push(id.clone());
        }
    })?;
    Ok(serde_json::json!({ "id": id, "ignored": p.ignored, "ignoredUpdates": s.ignored_updates }))
}
