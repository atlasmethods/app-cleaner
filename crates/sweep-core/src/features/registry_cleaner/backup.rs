//! Backups made before anything is changed, and their restore.
//!
//! Layout under `<data>/backups/`:
//!
//! ```text
//! registry-<unix ts>[-n]/   manifest.json + 000.reg, 001.reg, ... (one `reg export` per key)
//! config-<unix ts>[-n]/     manifest.json + files/000, files/001, ... (byte copies)
//! ```
//!
//! The manifest is written last, after every copy / export succeeded; a folder without a
//! manifest is an aborted attempt and is ignored (and cleaned up by the code that made it).

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ctx::{Ctx, Os};
use crate::elevate::run_privileged;
use crate::error::{ApiError, Result};
use crate::fsutil::{atomic_write, now_rfc3339, now_unix};
use crate::job::Job;
use crate::pkgutil::{path_size, summarize, valid_pkg_name};
use crate::safety::{ExcludeSet, SafeDeleter};

use super::model::{Issue, PkgManager};
use super::regaccess::{split_reg_path, Root};
use super::regcmd::{run_direct, run_privileged_batch, RegCmd};
use super::unixscan::is_user_path;

pub const KIND_REGISTRY: &str = "registry";
pub const KIND_CONFIG: &str = "config";
const MANIFEST: &str = "manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestIssue {
    pub id: String,
    pub category: String,
    pub description: String,
    pub location: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

impl From<&Issue> for ManifestIssue {
    fn from(i: &Issue) -> Self {
        ManifestIssue {
            id: i.id.clone(),
            category: i.category.clone(),
            description: i.description.clone(),
            location: i.location.clone(),
            value: i.value.clone(),
        }
    }
}

/// One exported registry key (`file` is relative to the backup folder).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeyEntry {
    pub file: String,
    /// `HKLM\SOFTWARE\...`
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub original_path: String,
    /// `file` or `symlink`
    pub kind: String,
    pub mode: u32,
    /// Copy inside the backup folder (`files/000`), for regular files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_target: Option<String>,
    #[serde(default)]
    pub size_bytes: u64,
    /// Outside the home directory: put back with administrator rights.
    #[serde(default)]
    pub system: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PackageEntry {
    pub manager: PkgManager,
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    pub kind: String,
    pub platform: String,
    pub created_at: String,
    pub created_at_unix: u64,
    pub issues: Vec<ManifestIssue>,
    #[serde(default)]
    pub keys: Vec<KeyEntry>,
    #[serde(default)]
    pub files: Vec<FileEntry>,
    #[serde(default)]
    pub packages: Vec<PackageEntry>,
}

impl Manifest {
    pub fn new(kind: &str, os: Os, issues: Vec<ManifestIssue>) -> Manifest {
        Manifest {
            version: 1,
            kind: kind.to_string(),
            platform: platform_name(os).to_string(),
            created_at: now_rfc3339(),
            created_at_unix: now_unix(),
            issues,
            keys: Vec::new(),
            files: Vec::new(),
            packages: Vec::new(),
        }
    }
}

pub fn platform_name(os: Os) -> &'static str {
    match os {
        Os::Windows => "windows",
        Os::Linux => "linux",
        Os::MacOs => "macos",
    }
}

pub fn backups_dir(ctx: &Ctx) -> PathBuf {
    ctx.env.data_dir.join("backups")
}

// ---------------------------------------------------------------- names

fn digits(s: &str) -> bool {
    !s.is_empty() && s.len() <= 12 && s.bytes().all(|b| b.is_ascii_digit())
}

/// `registry-1700000000` / `config-1700000000-2`.
pub fn valid_backup_id(id: &str) -> bool {
    let Some(rest) = id
        .strip_prefix("registry-")
        .or_else(|| id.strip_prefix("config-"))
    else {
        return false;
    };
    let mut it = rest.split('-');
    let (first, second, third) = (it.next(), it.next(), it.next());
    first.is_some_and(digits) && second.map_or(true, digits) && third.is_none()
}

/// Every name in the backups folder that ClearSweep creates, and what it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupKind {
    Registry,
    Config,
    /// `uninstall-<ts>.reg` (a single exported Uninstall key)
    UninstallEntry,
    /// `drivers-<ts>/` (exported driver packages)
    Drivers,
}

pub fn classify_name(name: &str) -> Option<BackupKind> {
    if valid_backup_id(name) {
        return Some(if name.starts_with("registry-") {
            BackupKind::Registry
        } else {
            BackupKind::Config
        });
    }
    if let Some(rest) = name
        .strip_prefix("uninstall-")
        .and_then(|r| r.strip_suffix(".reg"))
    {
        let mut it = rest.split('-');
        let (first, second, third) = (it.next(), it.next(), it.next());
        if first.is_some_and(digits) && second.map_or(true, digits) && third.is_none() {
            return Some(BackupKind::UninstallEntry);
        }
    }
    if let Some(rest) = name.strip_prefix("drivers-") {
        if digits(rest) {
            return Some(BackupKind::Drivers);
        }
    }
    None
}

/// Create a fresh, empty backup folder `<prefix>-<ts>` (never reusing a name).
pub fn new_backup_dir(ctx: &Ctx, prefix: &str) -> Result<(String, PathBuf)> {
    let base = backups_dir(ctx);
    fs::create_dir_all(&base)?;
    let ts = now_unix();
    for n in 0..1000u32 {
        let id = if n == 0 {
            format!("{prefix}-{ts}")
        } else {
            format!("{prefix}-{ts}-{n}")
        };
        let dir = base.join(&id);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(ApiError::io("could not choose a backup folder name"))
}

pub fn write_manifest(dir: &Path, m: &Manifest) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(m)?;
    atomic_write(&dir.join(MANIFEST), &bytes)?;
    Ok(())
}

pub fn read_manifest(dir: &Path) -> Result<Manifest> {
    let bytes = fs::read(dir.join(MANIFEST))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::io(format!("the backup manifest is damaged: {e}")))
}

// ---------------------------------------------------------------- deleting

/// A deleter confined to `<data>/backups`. The default protected set covers ClearSweep's whole
/// data folder (settings, history), which would refuse everything here, so the check runs with
/// the data folder taken out of the protected set; confinement to the backups folder, strict
/// name validation and `SafeDeleter`'s own checks (below the base, never a symlink target)
/// still apply.
fn backup_deleter(ctx: &Ctx) -> Result<SafeDeleter> {
    let mut env = ctx.env.clone();
    env.data_dir = env.temp_dir.join("clearsweep-no-such-data-dir");
    SafeDeleter::new(&env, &backups_dir(ctx), ExcludeSet::empty())
}

/// Remove one backup (folder or file) created by ClearSweep, by name.
pub fn delete_backup_entry(ctx: &Ctx, name: &str) -> Result<u64> {
    if classify_name(name).is_none() {
        return Err(ApiError::invalid_params(format!("`{name}` is not a ClearSweep backup")));
    }
    let path = backups_dir(ctx).join(name);
    let meta = fs::symlink_metadata(&path)
        .map_err(|_| ApiError::not_found(format!("backup `{name}` does not exist")))?;
    if meta.file_type().is_symlink() {
        return Err(ApiError::permission_denied("refusing to delete a symbolic link"));
    }
    let deleter = backup_deleter(ctx)?;
    let mut freed = 0u64;
    let fail = |e: crate::safety::SafeError, p: &Path| ApiError::io(format!("{}: {}", p.display(), e.message()));
    if meta.is_dir() {
        // contents first, then the folders themselves
        for e in walkdir::WalkDir::new(&path)
            .follow_links(false)
            .contents_first(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if e.file_type().is_dir() {
                deleter.remove_empty_dir(e.path()).map_err(|er| fail(er, e.path()))?;
            } else {
                freed += deleter.remove_file(e.path()).map_err(|er| fail(er, e.path()))?;
            }
        }
    } else {
        freed += deleter.remove_file(&path).map_err(|er| fail(er, &path))?;
    }
    Ok(freed)
}

// ---------------------------------------------------------------- listing

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub id: String,
    pub created_at: String,
    pub issue_count: usize,
    pub size_bytes: u64,
    pub platform: String,
    pub kind: String,
}

pub fn list(ctx: &Ctx) -> Vec<BackupInfo> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(backups_dir(ctx)) else {
        return out;
    };
    for e in rd.filter_map(|e| e.ok()) {
        let name = e.file_name().to_string_lossy().into_owned();
        if !valid_backup_id(&name) || !e.path().is_dir() {
            continue;
        }
        let Ok(m) = read_manifest(&e.path()) else {
            continue;
        };
        out.push(BackupInfo {
            id: name,
            created_at: m.created_at.clone(),
            issue_count: m.issues.len(),
            size_bytes: path_size(&e.path()),
            platform: m.platform.clone(),
            kind: m.kind.clone(),
        });
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
    out
}

// ---------------------------------------------------------------- file capture (unix)

#[cfg(unix)]
pub fn mode_of(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o7777
}
#[cfg(not(unix))]
pub fn mode_of(_meta: &fs::Metadata) -> u32 {
    0o644
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(mode))
}
#[cfg(not(unix))]
fn set_mode(_p: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn make_symlink(target: &str, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}
#[cfg(not(unix))]
fn make_symlink(_target: &str, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("symbolic links are not supported here"))
}

/// Copy `src` (a regular file or a symlink) into the backup and describe it. Directories and
/// special files are refused.
pub fn capture_path(ctx: &Ctx, dir: &Path, index: usize, src: &Path) -> Result<FileEntry> {
    let meta = fs::symlink_metadata(src)?;
    let system = !is_user_path(ctx, src);
    let original_path = src.to_string_lossy().into_owned();
    if meta.file_type().is_symlink() {
        let target = fs::read_link(src)?;
        return Ok(FileEntry {
            original_path,
            kind: "symlink".into(),
            mode: 0o777,
            stored: None,
            link_target: Some(target.to_string_lossy().into_owned()),
            size_bytes: 0,
            system,
        });
    }
    if !meta.is_file() {
        return Err(ApiError::invalid_params(format!(
            "{} is not a regular file or link",
            src.display()
        )));
    }
    let rel = format!("files/{index:03}");
    let dest = dir.join(&rel);
    fs::create_dir_all(dest.parent().unwrap())?;
    fs::copy(src, &dest)?;
    // The copy must be byte-identical before we let anything be deleted.
    if fs::metadata(&dest)?.len() != meta.len() {
        return Err(ApiError::io(format!("could not copy {} completely", src.display())));
    }
    Ok(FileEntry {
        original_path,
        kind: "file".into(),
        mode: mode_of(&meta),
        stored: Some(rel),
        link_target: None,
        size_bytes: meta.len(),
        system,
    })
}

// ---------------------------------------------------------------- restore

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreItem {
    pub target: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    pub id: String,
    pub ok: bool,
    pub restored: usize,
    pub failed: usize,
    pub results: Vec<RestoreItem>,
}

fn item(target: impl Into<String>, r: std::result::Result<(), String>) -> RestoreItem {
    match r {
        Ok(()) => RestoreItem {
            target: target.into(),
            ok: true,
            error: None,
        },
        Err(e) => RestoreItem {
            target: target.into(),
            ok: false,
            error: Some(e),
        },
    }
}

/// Where a file may be put back: inside the home directory (except secrets folders), or one of
/// the few system folders the cleaner works on. The manifest is data on disk, so its paths are
/// validated again instead of trusted.
pub fn restore_path_allowed(ctx: &Ctx, p: &Path) -> bool {
    if !p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return false;
    }
    if crate::safety::is_protected(&ctx.env, p) {
        return false;
    }
    if p.starts_with(&ctx.env.home) {
        return !(p.starts_with(ctx.env.home.join(".ssh")) || p.starts_with(ctx.env.home.join(".gnupg")));
    }
    [
        "/usr/share/applications",
        "/usr/local/share/applications",
        "/usr/local/bin",
    ]
    .iter()
    .any(|d| p.starts_with(ctx.env.sys_path(d)))
}

fn restore_user_file(dir: &Path, e: &FileEntry, dest: &Path) -> std::result::Result<(), String> {
    let err = |x: std::io::Error| x.to_string();
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(err)?;
    }
    match e.kind.as_str() {
        "file" => {
            let stored = e.stored.as_deref().ok_or("the backup has no copy of this file")?;
            if stored.contains("..") || stored.starts_with('/') {
                return Err("the backup entry is damaged".into());
            }
            let bytes = fs::read(dir.join(stored)).map_err(err)?;
            match fs::symlink_metadata(dest) {
                Ok(m) if m.file_type().is_symlink() || m.is_dir() => {
                    return Err("something else is at the original location now".into());
                }
                _ => {}
            }
            atomic_write(dest, &bytes).map_err(err)?;
            set_mode(dest, e.mode).map_err(err)?;
            Ok(())
        }
        "symlink" => {
            let target = e.link_target.as_deref().ok_or("the backup has no link target")?;
            match fs::symlink_metadata(dest) {
                Ok(m) if m.file_type().is_symlink() => {
                    if fs::read_link(dest).ok().as_deref() == Some(Path::new(target)) {
                        return Ok(());
                    }
                    fs::remove_file(dest).map_err(err)?;
                }
                Ok(_) => return Err("a file is at the link's original location now".into()),
                Err(_) => {}
            }
            make_symlink(target, dest).map_err(err)
        }
        other => Err(format!("unknown entry kind `{other}`")),
    }
}

fn restore_system_file(ctx: &Ctx, dir: &Path, e: &FileEntry, dest: &Path) -> std::result::Result<(), String> {
    let dest_s = dest.to_string_lossy().into_owned();
    let out = match e.kind.as_str() {
        "file" => {
            let stored = e.stored.as_deref().ok_or("the backup has no copy of this file")?;
            if stored.contains("..") || stored.starts_with('/') {
                return Err("the backup entry is damaged".into());
            }
            let src = dir.join(stored).to_string_lossy().into_owned();
            let mode = format!("{:o}", e.mode & 0o7777);
            run_privileged(ctx, "install", &["-m", &mode, &src, &dest_s])
        }
        "symlink" => {
            let target = e.link_target.as_deref().ok_or("the backup has no link target")?;
            if target.starts_with('-') && !target.contains('/') {
                return Err("unsafe link target".into());
            }
            run_privileged(ctx, "ln", &["-sfn", "--", target, &dest_s])
        }
        other => return Err(format!("unknown entry kind `{other}`")),
    };
    match out {
        Ok(o) if o.success() => Ok(()),
        Ok(o) => Err(format!("could not restore: {}", summarize(&o))),
        Err(e) => Err(e.message),
    }
}

fn restore_packages(ctx: &Ctx, packages: &[PackageEntry]) -> Vec<RestoreItem> {
    let mut out = Vec::new();
    for mgr in [PkgManager::Apt, PkgManager::Dnf, PkgManager::Pacman] {
        let names: Vec<&str> = packages
            .iter()
            .filter(|p| p.manager == mgr && valid_pkg_name(&p.name))
            .map(|p| p.name.as_str())
            .collect();
        if names.is_empty() {
            continue;
        }
        let (prog, mut args): (&str, Vec<&str>) = match mgr {
            PkgManager::Apt => ("apt-get", vec!["install", "-y", "--"]),
            PkgManager::Dnf => ("dnf", vec!["install", "-y", "--"]),
            PkgManager::Pacman => ("pacman", vec!["-S", "--noconfirm", "--needed", "--"]),
        };
        args.extend(&names);
        let r = match run_privileged(ctx, prog, &args) {
            Ok(o) if o.success() => Ok(()),
            Ok(o) => Err(format!("{prog} failed: {}", summarize(&o))),
            Err(e) => Err(e.message),
        };
        for n in names {
            out.push(item(format!("{} package {n}", mgr.name()), r.clone()));
        }
    }
    out
}

/// Put everything in backup `id` back.
pub fn restore(ctx: &Ctx, id: &str, job: &Job) -> Result<RestoreOutcome> {
    if !valid_backup_id(id) {
        return Err(ApiError::invalid_params(format!("`{id}` is not a backup id")));
    }
    let dir = backups_dir(ctx).join(id);
    if fs::symlink_metadata(&dir).is_err() {
        return Err(ApiError::not_found(format!("backup `{id}` does not exist")));
    }
    let m = read_manifest(&dir)?;
    let mut results: Vec<RestoreItem> = Vec::new();

    if !m.keys.is_empty() {
        if ctx.env.os != Os::Windows || m.platform != "windows" {
            return Err(ApiError::unsupported(
                "This backup contains Windows registry keys and can only be restored on Windows",
            ));
        }
        // HKLM keys need administrator rights: one prompt for all of them.
        let mut cmds = Vec::new();
        let mut admin = Vec::new();
        for k in &m.keys {
            let Some((root, _)) = split_reg_path(&k.key) else {
                results.push(item(&k.key, Err("unrecognised registry path".into())));
                continue;
            };
            if k.file.contains(['/', '\\']) || k.file.contains("..") {
                results.push(item(&k.key, Err("the backup entry is damaged".into())));
                continue;
            }
            let file = dir.join(&k.file);
            if !file.is_file() {
                results.push(item(&k.key, Err("the exported file is missing".into())));
                continue;
            }
            let cmd = RegCmd::import(&file.to_string_lossy());
            if root == Root::Hklm {
                admin.push(k.key.clone());
                cmds.push(cmd);
            } else {
                job.check_cancelled()?;
                let r = run_direct(ctx, &cmd);
                results.push(item(&k.key, if r.ok { Ok(()) } else { Err(r.message) }));
            }
        }
        if !cmds.is_empty() {
            job.check_cancelled()?;
            let rs = run_privileged_batch(ctx, &cmds);
            for (key, r) in admin.into_iter().zip(rs) {
                results.push(item(key, if r.ok { Ok(()) } else { Err(r.message) }));
            }
        }
    }

    for e in &m.files {
        job.check_cancelled()?;
        let dest = PathBuf::from(&e.original_path);
        if !restore_path_allowed(ctx, &dest) {
            results.push(item(&e.original_path, Err("this location is not one ClearSweep restores to".into())));
            continue;
        }
        let r = if is_user_path(ctx, &dest) {
            restore_user_file(&dir, e, &dest)
        } else {
            restore_system_file(ctx, &dir, e, &dest)
        };
        results.push(item(&e.original_path, r));
    }

    if !m.packages.is_empty() {
        job.check_cancelled()?;
        results.extend(restore_packages(ctx, &m.packages));
    }

    let failed = results.iter().filter(|r| !r.ok).count();
    Ok(RestoreOutcome {
        id: id.to_string(),
        ok: failed == 0,
        restored: results.len() - failed,
        failed,
        results,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::runner::MockRunner;

    fn ctx() -> (tempfile::TempDir, Ctx) {
        let d = tempfile::tempdir().unwrap();
        let c = Ctx::test(d.path(), MockRunner::new());
        fs::create_dir_all(&c.env.home).unwrap();
        fs::create_dir_all(&c.env.root).unwrap();
        (d, c)
    }

    #[test]
    fn backup_id_validation() {
        for ok in ["registry-1700000000", "config-1700000000", "config-1700000000-3"] {
            assert!(valid_backup_id(ok), "{ok}");
        }
        for bad in [
            "", "registry-", "registry-x", "config-1-", "config-1-2-3", "config-1/../x", "../config-1", "uninstall-1.reg",
            "registry-1700000000000000", "config--1", "Config-1",
        ] {
            assert!(!valid_backup_id(bad), "{bad}");
        }
    }

    #[test]
    fn name_classification() {
        assert_eq!(classify_name("registry-5"), Some(BackupKind::Registry));
        assert_eq!(classify_name("config-5-1"), Some(BackupKind::Config));
        assert_eq!(classify_name("uninstall-1700000000.reg"), Some(BackupKind::UninstallEntry));
        assert_eq!(classify_name("uninstall-1700000000-2.reg"), Some(BackupKind::UninstallEntry));
        assert_eq!(classify_name("drivers-1700000000"), Some(BackupKind::Drivers));
        for bad in ["uninstall-1.txt", "uninstall-.reg", "drivers-", "drivers-1/x", "settings.json", "..", "uninstall-1-2-3.reg"] {
            assert_eq!(classify_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn capture_and_restore_round_trip_with_modes() {
        let (_d, c) = ctx();
        let (id, dir) = new_backup_dir(&c, KIND_CONFIG).unwrap();
        let apps = c.env.user_data_dir.join("applications");
        fs::create_dir_all(&apps).unwrap();
        let f = apps.join("x.desktop");
        fs::write(&f, b"[Desktop Entry]\r\nExec=gone\r\n\xff\xfe binary-ish").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&f, fs::Permissions::from_mode(0o750)).unwrap();
        }
        let l = apps.join("link.desktop");
        std::os::unix::fs::symlink("../nowhere/x.desktop", &l).unwrap();
        let mut m = Manifest::new(KIND_CONFIG, Os::Linux, vec![]);
        m.files.push(capture_path(&c, &dir, 0, &f).unwrap());
        m.files.push(capture_path(&c, &dir, 1, &l).unwrap());
        write_manifest(&dir, &m).unwrap();
        assert_eq!(m.files[0].mode, 0o750);
        assert_eq!(m.files[1].kind, "symlink");
        let original = fs::read(&f).unwrap();

        fs::remove_file(&f).unwrap();
        fs::remove_file(&l).unwrap();
        fs::remove_dir(&apps).unwrap(); // the folder is recreated too

        let out = restore(&c, &id, &Job::detached()).unwrap();
        assert!(out.ok, "{out:?}");
        assert_eq!(out.restored, 2);
        assert_eq!(fs::read(&f).unwrap(), original);
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&f).unwrap().permissions().mode() & 0o7777, 0o750);
        }
        assert_eq!(fs::read_link(&l).unwrap(), Path::new("../nowhere/x.desktop"));
        // restoring twice is harmless
        let again = restore(&c, &id, &Job::detached()).unwrap();
        assert!(again.ok);
    }

    #[test]
    fn restore_refuses_odd_locations_and_replaced_paths() {
        let (_d, c) = ctx();
        let (id, dir) = new_backup_dir(&c, KIND_CONFIG).unwrap();
        fs::create_dir_all(dir.join("files")).unwrap();
        fs::write(dir.join("files/000"), b"x").unwrap();
        let mut m = Manifest::new(KIND_CONFIG, Os::Linux, vec![]);
        for p in ["/etc/passwd", "relative/x", &format!("{}/../etc/x", c.env.home.display())] {
            m.files.push(FileEntry {
                original_path: p.into(),
                kind: "file".into(),
                mode: 0o644,
                stored: Some("files/000".into()),
                link_target: None,
                size_bytes: 1,
                system: false,
            });
        }
        // inside home but secrets
        m.files.push(FileEntry {
            original_path: c.env.home.join(".ssh/authorized_keys").to_string_lossy().into_owned(),
            kind: "file".into(),
            mode: 0o644,
            stored: Some("files/000".into()),
            link_target: None,
            size_bytes: 1,
            system: false,
        });
        // a damaged stored path
        m.files.push(FileEntry {
            original_path: c.env.home.join("ok.txt").to_string_lossy().into_owned(),
            kind: "file".into(),
            mode: 0o644,
            stored: Some("../../../etc/shadow".into()),
            link_target: None,
            size_bytes: 1,
            system: false,
        });
        write_manifest(&dir, &m).unwrap();
        let out = restore(&c, &id, &Job::detached()).unwrap();
        assert!(!out.ok);
        assert_eq!(out.failed, 5);
        assert!(!c.env.home.join("ok.txt").exists());
        assert!(!c.env.home.join(".ssh").exists());
    }

    #[test]
    fn restore_will_not_replace_a_link_or_directory() {
        let (_d, c) = ctx();
        let (id, dir) = new_backup_dir(&c, KIND_CONFIG).unwrap();
        let target = c.env.home.join("f.txt");
        fs::write(&target, b"orig").unwrap();
        let mut m = Manifest::new(KIND_CONFIG, Os::Linux, vec![]);
        m.files.push(capture_path(&c, &dir, 0, &target).unwrap());
        write_manifest(&dir, &m).unwrap();
        fs::remove_file(&target).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", &target).unwrap();
        let out = restore(&c, &id, &Job::detached()).unwrap();
        assert!(!out.ok);
        assert_eq!(fs::read_link(&target).unwrap(), Path::new("/etc/passwd"));
    }

    #[test]
    fn capture_refuses_directories() {
        let (_d, c) = ctx();
        let (_id, dir) = new_backup_dir(&c, KIND_CONFIG).unwrap();
        assert!(capture_path(&c, &dir, 0, &c.env.home).is_err());
    }

    #[test]
    fn listing_ignores_incomplete_and_foreign_folders() {
        let (_d, c) = ctx();
        let (id1, d1) = new_backup_dir(&c, KIND_CONFIG).unwrap();
        let mut m = Manifest::new(KIND_CONFIG, Os::Linux, vec![ManifestIssue { id: "a".into(), category: "x".into(), description: "d".into(), location: "l".into(), value: None }]);
        m.created_at = "2024-01-01T00:00:00Z".into();
        write_manifest(&d1, &m).unwrap();
        let (_id2, _d2) = new_backup_dir(&c, KIND_CONFIG).unwrap(); // no manifest: aborted attempt
        fs::create_dir_all(backups_dir(&c).join("random")).unwrap();
        fs::write(backups_dir(&c).join("uninstall-1.reg"), b"x").unwrap();
        let l = list(&c);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].id, id1);
        assert_eq!(l[0].issue_count, 1);
        assert_eq!(l[0].platform, "linux");
        assert!(l[0].size_bytes > 0);
    }

    #[test]
    fn deleting_backups_through_the_safe_deleter() {
        let (_d, c) = ctx();
        let (id, dir) = new_backup_dir(&c, KIND_CONFIG).unwrap();
        fs::create_dir_all(dir.join("files")).unwrap();
        fs::write(dir.join("files/000"), b"12345").unwrap();
        write_manifest(&dir, &Manifest::new(KIND_CONFIG, Os::Linux, vec![])).unwrap();
        // an unrelated file next to it survives
        fs::write(c.env.data_dir.join("settings.json"), b"{}").unwrap();
        let freed = delete_backup_entry(&c, &id).unwrap();
        assert!(freed >= 5);
        assert!(!dir.exists());
        assert!(c.env.data_dir.join("settings.json").exists());
        assert!(backups_dir(&c).exists());
        // legacy single-file and folder backups
        fs::write(backups_dir(&c).join("uninstall-1700000000.reg"), b"reg").unwrap();
        fs::create_dir_all(backups_dir(&c).join("drivers-1700000000/x")).unwrap();
        fs::write(backups_dir(&c).join("drivers-1700000000/x/a.inf"), b"inf").unwrap();
        delete_backup_entry(&c, "uninstall-1700000000.reg").unwrap();
        delete_backup_entry(&c, "drivers-1700000000").unwrap();
        assert!(fs::read_dir(backups_dir(&c)).unwrap().next().is_none());
    }

    #[test]
    fn deleting_refuses_bad_names_links_and_missing() {
        let (_d, c) = ctx();
        fs::create_dir_all(backups_dir(&c)).unwrap();
        for bad in ["..", "../data", "settings.json", "config-1/../..", "registry-x", ""] {
            assert!(delete_backup_entry(&c, bad).is_err(), "{bad}");
        }
        assert_eq!(delete_backup_entry(&c, "config-99").unwrap_err().code, crate::error::ErrorCode::NotFound);
        // a symlink posing as a backup is not followed or removed
        let victim = c.env.home.join("victim");
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep"), b"k").unwrap();
        std::os::unix::fs::symlink(&victim, backups_dir(&c).join("config-7")).unwrap();
        assert!(delete_backup_entry(&c, "config-7").is_err());
        assert!(victim.join("keep").exists());
    }
}
