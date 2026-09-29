//! Applying fixes: back everything up first, abort on any backup failure, then change.
//!
//! The caller only names issue ids; the actions come from a scan run right now, so anything
//! that is not in the fresh scan is refused.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ctx::{Ctx, Os};
use crate::elevate::run_privileged;
use crate::error::{ApiError, Result};
use crate::fsutil::atomic_write;
use crate::job::{Job, ProgressEvent};
use crate::pkgutil::{summarize, valid_pkg_name};
use crate::safety::{ExcludeSet, SafeDeleter};

use super::backup::{
    self, capture_path, new_backup_dir, write_manifest, KeyEntry, Manifest, ManifestIssue,
    PackageEntry, KIND_CONFIG, KIND_REGISTRY,
};
use super::mimeapps;
use super::model::{Action, Found, PkgManager};
use super::regaccess::{reg_path, Root};
use super::regcmd::{run_direct, run_privileged_batch, RegCmd};
use super::unixscan::{is_user_path, parse_apt_autoremove};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueResult {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl IssueResult {
    fn ok(id: &str) -> Self {
        IssueResult {
            id: id.to_string(),
            ok: true,
            error: None,
        }
    }
    fn fail(id: &str, e: impl Into<String>) -> Self {
        IssueResult {
            id: id.to_string(),
            ok: false,
            error: Some(e.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FixOutcome {
    pub backup_id: Option<String>,
    pub fixed: usize,
    pub failed: usize,
    pub results: Vec<IssueResult>,
}

pub const NOT_IN_SCAN: &str = "This item was not found by a fresh scan (it may already be fixed).";

/// Fix `ids`, which must all come from `current` (a scan made moments ago).
pub fn apply(ctx: &Ctx, current: Vec<Found>, ids: &[String], job: &Job) -> Result<FixOutcome> {
    let mut by_id: HashMap<String, Found> = current
        .into_iter()
        .map(|f| (f.issue.id.clone(), f))
        .collect();
    let mut selected: Vec<Found> = Vec::new();
    let mut results: Vec<IssueResult> = Vec::new();
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id.clone()) {
            continue;
        }
        match by_id.remove(id) {
            Some(f) => selected.push(f),
            None => results.push(IssueResult::fail(id, NOT_IN_SCAN)),
        }
    }
    if selected.is_empty() {
        let failed = results.len();
        return Ok(FixOutcome {
            backup_id: None,
            fixed: 0,
            failed,
            results,
        });
    }
    job.check_cancelled()?;
    let (backup_id, applied) = match ctx.env.os {
        Os::Windows => fix_windows(ctx, &selected, job)?,
        _ => fix_unix(ctx, &selected, job)?,
    };
    results.extend(applied);
    let fixed = results.iter().filter(|r| r.ok).count();
    let failed = results.len() - fixed;
    Ok(FixOutcome {
        backup_id: Some(backup_id),
        fixed,
        failed,
        results,
    })
}

/// Best-effort removal of a backup folder that never got a manifest.
fn abandon(ctx: &Ctx, id: &str) {
    let _ = backup::delete_backup_entry(ctx, id);
}

// ---------------------------------------------------------------- Windows

fn key_of_action(a: &Action) -> Option<(Root, &str)> {
    match a {
        Action::RegDeleteValue { root, key, .. } | Action::RegDeleteKey { root, key } => {
            Some((*root, key.as_str()))
        }
        _ => None,
    }
}

/// The keys to export: each affected key once, without keys already inside another exported one.
pub fn keys_to_export(selected: &[Found]) -> Vec<(Root, String)> {
    let mut keys: Vec<(Root, String)> = Vec::new();
    for f in selected {
        if let Some((r, k)) = key_of_action(&f.action) {
            if !keys.iter().any(|(r2, k2)| *r2 == r && k2.eq_ignore_ascii_case(k)) {
                keys.push((r, k.to_string()));
            }
        }
    }
    let lower: Vec<String> = keys.iter().map(|(_, k)| k.to_ascii_lowercase()).collect();
    keys.iter()
        .enumerate()
        .filter(|(i, (r, _))| {
            !keys.iter().enumerate().any(|(j, (r2, _))| {
                j != *i && r2 == r && lower[*i].starts_with(&format!("{}\\", lower[j]))
            })
        })
        .map(|(_, k)| k.clone())
        .collect()
}

fn export_keys(ctx: &Ctx, dir: &Path, keys: &[(Root, String)], job: &Job) -> Result<Vec<KeyEntry>> {
    let mut out = Vec::new();
    for (i, (root, key)) in keys.iter().enumerate() {
        job.check_cancelled()?;
        job.progress(
            ProgressEvent::new("backup")
                .message("Backing up registry keys".to_string())
                .counts(i as u64, keys.len() as u64),
        );
        let file = format!("{i:03}.reg");
        let path = reg_path(*root, key);
        let file_path = dir.join(&file);
        let file_s = file_path.to_string_lossy().into_owned();
        let r = ctx.runner.run("reg", &["export", &path, &file_s, "/y"])?;
        let written = fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
        if !r.success() || written == 0 {
            return Err(ApiError::io(format!(
                "Could not back up {path} first, so nothing was changed. {}",
                summarize(&r)
            )));
        }
        out.push(KeyEntry { file, key: path });
    }
    Ok(out)
}

fn fix_windows(ctx: &Ctx, selected: &[Found], job: &Job) -> Result<(String, Vec<IssueResult>)> {
    let keys = keys_to_export(selected);
    let (id, dir) = new_backup_dir(ctx, KIND_REGISTRY)?;
    let entries = match export_keys(ctx, &dir, &keys, job) {
        Ok(e) => e,
        Err(e) => {
            abandon(ctx, &id);
            return Err(e);
        }
    };
    let mut m = Manifest::new(
        KIND_REGISTRY,
        Os::Windows,
        selected.iter().map(|f| ManifestIssue::from(&f.issue)).collect(),
    );
    m.keys = entries;
    if let Err(e) = write_manifest(&dir, &m) {
        abandon(ctx, &id);
        return Err(e);
    }

    // Everything is saved. Per-user keys are changed directly, machine keys in one elevated batch.
    let mut results: HashMap<String, IssueResult> = HashMap::new();
    let mut admin: Vec<(&Found, RegCmd)> = Vec::new();
    let total = selected.len() as u64;
    let mut cancelled = false;
    for (i, f) in selected.iter().enumerate() {
        let cmd = match &f.action {
            Action::RegDeleteValue { root, key, name } => {
                (*root, RegCmd::delete_value(&reg_path(*root, key), name))
            }
            Action::RegDeleteKey { root, key } => (*root, RegCmd::delete_key(&reg_path(*root, key))),
            _ => {
                results.insert(f.issue.id.clone(), IssueResult::fail(&f.issue.id, "not a registry item"));
                continue;
            }
        };
        if cancelled || job.is_cancelled() {
            cancelled = true;
            results.insert(f.issue.id.clone(), IssueResult::fail(&f.issue.id, "Cancelled before this item was changed."));
            continue;
        }
        if cmd.0 == Root::Hklm {
            admin.push((f, cmd.1));
        } else {
            job.progress(
                ProgressEvent::new("fix")
                    .message("Fixing registry entries".to_string())
                    .counts(i as u64, total),
            );
            let r = run_direct(ctx, &cmd.1);
            results.insert(
                f.issue.id.clone(),
                if r.ok {
                    IssueResult::ok(&f.issue.id)
                } else {
                    IssueResult::fail(&f.issue.id, r.message)
                },
            );
        }
    }
    if !admin.is_empty() && !cancelled {
        job.progress(ProgressEvent::new("fix").message("Waiting for administrator approval".to_string()));
        let cmds: Vec<RegCmd> = admin.iter().map(|(_, c)| c.clone()).collect();
        let rs = run_privileged_batch(ctx, &cmds);
        for ((f, _), r) in admin.iter().zip(rs) {
            results.insert(
                f.issue.id.clone(),
                if r.ok {
                    IssueResult::ok(&f.issue.id)
                } else {
                    IssueResult::fail(&f.issue.id, r.message)
                },
            );
        }
    } else {
        for (f, _) in &admin {
            results.insert(f.issue.id.clone(), IssueResult::fail(&f.issue.id, "Cancelled before this item was changed."));
        }
    }
    let ordered = selected
        .iter()
        .map(|f| {
            results
                .remove(&f.issue.id)
                .unwrap_or_else(|| IssueResult::fail(&f.issue.id, "no result"))
        })
        .collect();
    Ok((id, ordered))
}

// ---------------------------------------------------------------- Linux / macOS

/// Files the backup must contain for `f`.
fn files_of(f: &Found) -> Vec<PathBuf> {
    match &f.action {
        Action::RemoveFile { path, also, .. } => {
            let mut v = vec![path.clone()];
            v.extend(also.iter().cloned());
            v
        }
        Action::MimeRemove { file, .. } => vec![file.clone()],
        _ => Vec::new(),
    }
}

fn fix_unix(ctx: &Ctx, selected: &[Found], job: &Job) -> Result<(String, Vec<IssueResult>)> {
    let (id, dir) = new_backup_dir(ctx, KIND_CONFIG)?;
    let mut m = Manifest::new(
        KIND_CONFIG,
        ctx.env.os,
        selected.iter().map(|f| ManifestIssue::from(&f.issue)).collect(),
    );
    let mut capture = || -> Result<()> {
        let mut done: HashSet<PathBuf> = HashSet::new();
        for f in selected {
            job.check_cancelled()?;
            for p in files_of(f) {
                if !done.insert(p.clone()) {
                    continue;
                }
                job.progress(
                    ProgressEvent::new("backup")
                        .message("Backing up files".to_string())
                        .counts(done.len() as u64, selected.len() as u64),
                );
                m.files.push(capture_path(ctx, &dir, m.files.len(), &p)?);
            }
            if let Action::RemovePackage {
                manager,
                package,
                version,
            } = &f.action
            {
                m.packages.push(PackageEntry {
                    manager: *manager,
                    name: package.clone(),
                    version: version.clone(),
                });
            }
        }
        write_manifest(&dir, &m)
    };
    if let Err(e) = capture() {
        abandon(ctx, &id);
        return Err(ApiError::new(
            e.code,
            format!("Could not back up first, so nothing was changed. {}", e.message),
        ));
    }

    let mut results: HashMap<String, IssueResult> = HashMap::new();
    let excludes = {
        let s = crate::features::settings::load(ctx);
        ExcludeSet::from_settings(&ctx.env, &s).unwrap_or_default()
    };
    let safety = std::sync::Arc::new(crate::safety::Safety::new(&ctx.env, excludes));

    // 1. user files and links
    let mut system_files: Vec<&Found> = Vec::new();
    let mut mime: HashMap<PathBuf, Vec<&Found>> = HashMap::new();
    let mut packages: Vec<&Found> = Vec::new();
    for f in selected {
        match &f.action {
            Action::RemoveFile { system: true, .. } => system_files.push(f),
            Action::RemoveFile { path, also, .. } => {
                let r = remove_user_file(&safety, path, also, job);
                results.insert(f.issue.id.clone(), to_result(&f.issue.id, r));
            }
            Action::MimeRemove { file, .. } => mime.entry(file.clone()).or_default().push(f),
            Action::RemovePackage { .. } => packages.push(f),
            _ => {
                results.insert(f.issue.id.clone(), IssueResult::fail(&f.issue.id, "not a file item"));
            }
        }
    }

    // 2. mimeapps.list edits (one write per file)
    for (file, items) in &mime {
        edit_mimeapps(file, items, &mut results);
    }

    // 3. system files: one elevated `rm`
    if !system_files.is_empty() {
        if job.is_cancelled() {
            for f in &system_files {
                results.insert(f.issue.id.clone(), IssueResult::fail(&f.issue.id, "Cancelled before this item was changed."));
            }
        } else {
            remove_system_files(ctx, &system_files, &mut results);
        }
    }

    // 4. packages
    if !packages.is_empty() {
        if job.is_cancelled() {
            for f in &packages {
                results.insert(f.issue.id.clone(), IssueResult::fail(&f.issue.id, "Cancelled before this item was changed."));
            }
        } else {
            remove_packages(ctx, &packages, &mut results);
        }
    }

    let ordered = selected
        .iter()
        .map(|f| {
            results
                .remove(&f.issue.id)
                .unwrap_or_else(|| IssueResult::fail(&f.issue.id, "no result"))
        })
        .collect();
    Ok((id, ordered))
}

fn to_result(id: &str, r: std::result::Result<(), String>) -> IssueResult {
    match r {
        Ok(()) => IssueResult::ok(id),
        Err(e) => IssueResult::fail(id, e),
    }
}

fn remove_user_file(
    safety: &std::sync::Arc<crate::safety::Safety>,
    path: &Path,
    also: &[PathBuf],
    job: &Job,
) -> std::result::Result<(), String> {
    if job.is_cancelled() {
        return Err("Cancelled before this item was changed.".into());
    }
    let parent = path.parent().ok_or("the item has no folder")?;
    let deleter = SafeDeleter::with_safety(safety.clone(), parent).map_err(|e| e.message)?;
    match deleter.remove_file(path) {
        Ok(_) => {}
        // already gone: the goal is reached
        Err(e) if e.is_not_found() => {}
        Err(e) => return Err(e.message()),
    }
    // The links that enabled a removed service are cleaned up on a best-effort basis.
    for l in also {
        if let Some(p) = l.parent() {
            if let Ok(d) = SafeDeleter::with_safety(safety.clone(), p) {
                let _ = d.remove_file(l);
            }
        }
    }
    Ok(())
}

fn edit_mimeapps(file: &Path, items: &[&Found], results: &mut HashMap<String, IssueResult>) {
    let fail_all = |results: &mut HashMap<String, IssueResult>, msg: &str| {
        for f in items {
            results.insert(f.issue.id.clone(), IssueResult::fail(&f.issue.id, msg));
        }
    };
    let meta = match fs::symlink_metadata(file) {
        Ok(m) if m.file_type().is_file() => m,
        Ok(_) => return fail_all(results, "The file is a link or folder now; left alone."),
        Err(e) => return fail_all(results, &e.to_string()),
    };
    let content = match fs::read_to_string(file) {
        Ok(c) => c,
        Err(e) => return fail_all(results, &e.to_string()),
    };
    let removals: Vec<(String, String, String)> = items
        .iter()
        .filter_map(|f| match &f.action {
            Action::MimeRemove {
                section,
                mime,
                desktop_id,
                ..
            } => Some((section.clone(), mime.clone(), desktop_id.clone())),
            _ => None,
        })
        .collect();
    let (new, n) = mimeapps::remove_ids(&content, &removals);
    if n == 0 {
        return fail_all(results, NOT_IN_SCAN);
    }
    if new != content {
        let write = atomic_write(file, new.as_bytes())
            .and_then(|_| restore_mode(file, &meta));
        if let Err(e) = write {
            return fail_all(results, &format!("could not write the file: {e}"));
        }
    }
    let after = mimeapps::associations(&new);
    for f in items {
        let ok = match &f.action {
            Action::MimeRemove {
                section,
                mime,
                desktop_id,
                ..
            } => !after
                .iter()
                .any(|a| a.section == *section && a.mime == *mime && a.ids.contains(desktop_id)),
            _ => false,
        };
        results.insert(
            f.issue.id.clone(),
            if ok {
                IssueResult::ok(&f.issue.id)
            } else {
                IssueResult::fail(&f.issue.id, "The entry could not be removed.")
            },
        );
    }
}

#[cfg(unix)]
fn restore_mode(file: &Path, orig: &fs::Metadata) -> std::io::Result<()> {
    fs::set_permissions(file, orig.permissions())
}
#[cfg(not(unix))]
fn restore_mode(_file: &Path, _orig: &fs::Metadata) -> std::io::Result<()> {
    Ok(())
}

fn remove_system_files(ctx: &Ctx, items: &[&Found], results: &mut HashMap<String, IssueResult>) {
    let mut paths: Vec<String> = Vec::new();
    let mut owner: Vec<(usize, &Found)> = Vec::new();
    for f in items {
        let Action::RemoveFile { path, .. } = &f.action else {
            continue;
        };
        if is_user_path(ctx, path) || !backup::restore_path_allowed(ctx, path) {
            results.insert(
                f.issue.id.clone(),
                IssueResult::fail(&f.issue.id, "This location is not one ClearSweep changes."),
            );
            continue;
        }
        owner.push((paths.len(), f));
        paths.push(path.to_string_lossy().into_owned());
    }
    if paths.is_empty() {
        return;
    }
    let mut args: Vec<&str> = vec!["-f", "--"];
    args.extend(paths.iter().map(String::as_str));
    let outcome = run_privileged(ctx, "rm", &args);
    for (i, f) in owner {
        let p = Path::new(&paths[i]);
        let gone = fs::symlink_metadata(p).is_err();
        let r = if gone {
            IssueResult::ok(&f.issue.id)
        } else {
            let why = match &outcome {
                Err(e) => e.message.clone(),
                Ok(o) => format!("could not remove it: {}", summarize(o)),
            };
            IssueResult::fail(&f.issue.id, why)
        };
        results.insert(f.issue.id.clone(), r);
    }
}

fn pkg_base(name: &str) -> &str {
    name.split(':').next().unwrap_or(name)
}

fn remove_packages(ctx: &Ctx, items: &[&Found], results: &mut HashMap<String, IssueResult>) {
    for mgr in [PkgManager::Apt, PkgManager::Dnf, PkgManager::Pacman] {
        let group: Vec<(&Found, &str)> = items
            .iter()
            .filter_map(|f| match &f.action {
                Action::RemovePackage { manager, package, .. } if *manager == mgr => {
                    Some((*f, package.as_str()))
                }
                _ => None,
            })
            .collect();
        if group.is_empty() {
            continue;
        }
        let mut names: Vec<&str> = Vec::new();
        for (f, n) in &group {
            if valid_pkg_name(n) {
                names.push(n);
            } else {
                results.insert(f.issue.id.clone(), IssueResult::fail(&f.issue.id, "unsafe package name"));
            }
        }
        if names.is_empty() {
            continue;
        }
        let fail_group = |results: &mut HashMap<String, IssueResult>, msg: String| {
            for (f, n) in &group {
                if names.contains(n) {
                    results.insert(f.issue.id.clone(), IssueResult::fail(&f.issue.id, msg.clone()));
                }
            }
        };
        // Removing must not drag anything else along: apt can simulate it.
        if mgr == PkgManager::Apt {
            let mut sim: Vec<&str> = vec!["-s", "remove", "--"];
            sim.extend(&names);
            match ctx.runner.run("apt-get", &sim) {
                Ok(o) if o.success() => {
                    let wanted: HashSet<&str> = names.iter().map(|n| pkg_base(n)).collect();
                    let extra: Vec<String> = parse_apt_autoremove(&o.stdout)
                        .into_iter()
                        .map(|(n, _)| n)
                        .filter(|n| !wanted.contains(pkg_base(n)))
                        .collect();
                    if !extra.is_empty() {
                        fail_group(
                            results,
                            format!("Removing these would also remove: {}. Nothing was removed.", extra.join(", ")),
                        );
                        continue;
                    }
                }
                Ok(o) => {
                    fail_group(results, format!("apt could not plan the removal: {}", summarize(&o)));
                    continue;
                }
                Err(e) => {
                    fail_group(results, e.message);
                    continue;
                }
            }
        }
        let (prog, mut args): (&str, Vec<&str>) = match mgr {
            PkgManager::Apt => ("apt-get", vec!["remove", "-y", "--"]),
            PkgManager::Dnf => ("dnf", vec!["remove", "-y", "--"]),
            PkgManager::Pacman => ("pacman", vec!["-Rns", "--noconfirm", "--"]),
        };
        args.extend(&names);
        match run_privileged(ctx, prog, &args) {
            Ok(o) if o.success() => {
                for (f, n) in &group {
                    if names.contains(n) {
                        results.insert(f.issue.id.clone(), IssueResult::ok(&f.issue.id));
                    }
                }
            }
            Ok(o) => fail_group(results, format!("{prog} failed: {}", summarize(&o))),
            Err(e) => fail_group(results, e.message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::{Issue, Severity};
    use super::*;

    fn found(root: Root, key: &str, name: Option<&str>) -> Found {
        let action = match name {
            Some(n) => Action::RegDeleteValue {
                root,
                key: key.into(),
                name: n.into(),
            },
            None => Action::RegDeleteKey {
                root,
                key: key.into(),
            },
        };
        Found {
            issue: Issue {
                id: format!("{key}|{name:?}"),
                category: "c".into(),
                description: String::new(),
                location: String::new(),
                value: None,
                data: None,
                severity: Severity::Low,
                needs_admin: false,
            },
            action,
        }
    }

    #[test]
    fn export_set_is_deduplicated_and_nested_keys_collapse() {
        let sel = vec![
            found(Root::Hklm, r"SOFTWARE\A", Some("v1")),
            found(Root::Hklm, r"software\a", Some("v2")),
            found(Root::Hklm, r"SOFTWARE\A\Sub", None),
            found(Root::Hkcu, r"SOFTWARE\A\Sub", None),
            found(Root::Hklm, r"SOFTWARE\AB", None),
            found(Root::Hklm, r"SOFTWARE\B\C", None),
        ];
        let keys = keys_to_export(&sel);
        assert_eq!(
            keys,
            vec![
                (Root::Hklm, r"SOFTWARE\A".to_string()),
                (Root::Hkcu, r"SOFTWARE\A\Sub".to_string()),
                (Root::Hklm, r"SOFTWARE\AB".to_string()),
                (Root::Hklm, r"SOFTWARE\B\C".to_string()),
            ]
        );
    }
}
