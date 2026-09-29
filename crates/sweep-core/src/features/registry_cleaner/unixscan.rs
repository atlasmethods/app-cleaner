//! "Config issues" on Linux and macOS: broken launchers, autostart entries, dangling links,
//! stale file associations, stale user services, launch agents and orphaned packages.
//!
//! Same rule as on Windows: when a target cannot be judged (unreadable, unresolvable variable,
//! removable media, ...) it is not reported.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ctx::{Ctx, Os};
use crate::error::{ApiError, Result};
use crate::job::Job;
use crate::pkgutil::valid_pkg_name;

use super::cmdline::Located;
use super::desktop::{self, ExecTarget};
use super::mimeapps;
use super::model::{Action, Found, PkgManager, Severity};

/// Cap on how many orphaned packages are offered.
const MAX_PACKAGES: usize = 500;
const MAX_FILE_BYTES: u64 = 1024 * 1024;

// ---------------------------------------------------------------- path helpers

/// Map an absolute path from a config file to the real location: paths inside the home
/// directory stay as they are, everything else goes through the (possibly redirected) root.
pub fn map_abs(ctx: &Ctx, abs: &str) -> PathBuf {
    let p = Path::new(abs);
    if p.starts_with(&ctx.env.home) {
        p.to_path_buf()
    } else {
        ctx.env.sys_path(abs)
    }
}

/// Does `p` live in the user's home (fixable without elevation)?
pub fn is_user_path(ctx: &Ctx, p: &Path) -> bool {
    p.starts_with(&ctx.env.home)
}

fn stat_abs(ctx: &Ctx, abs: &str) -> Located {
    match fs::metadata(map_abs(ctx, abs)) {
        Ok(_) => Located::Present,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Located::Missing(abs.to_string()),
        Err(_) => Located::Unknown,
    }
}

fn list_dir(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Directories a bare program name may live in, besides `PATH` (a launcher started from the
/// desktop often gets a longer PATH than this process).
fn extra_bin_dirs(ctx: &Ctx) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = [
        "/usr/local/sbin",
        "/usr/local/bin",
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
        "/snap/bin",
        "/usr/games",
        "/usr/local/games",
        "/opt/homebrew/bin",
        "/var/lib/flatpak/exports/bin",
        "/run/current-system/sw/bin",
    ]
    .iter()
    .map(|d| ctx.env.sys_path(d))
    .collect();
    for d in [
        ".local/bin",
        "bin",
        ".cargo/bin",
        "go/bin",
        ".nix-profile/bin",
        ".pyenv/shims",
        ".rbenv/shims",
        ".asdf/shims",
        ".volta/bin",
        ".bun/bin",
        ".deno/bin",
        ".dotnet/tools",
        ".local/share/flatpak/exports/bin",
    ] {
        v.push(ctx.env.home.join(d));
    }
    v
}

/// Find a program the way a launcher would.
pub fn locate_program(ctx: &Ctx, prog: &str, cwd: Option<&str>) -> Located {
    let prog = prog.trim();
    if prog.is_empty() || prog.contains(['$', '%', '\0', '`']) || prog.starts_with('~') {
        return Located::Unknown;
    }
    if prog.starts_with('/') {
        return stat_abs(ctx, prog);
    }
    if prog.contains('/') {
        // relative to the launcher's working directory, when it names one
        return match cwd {
            Some(c) if c.starts_with('/') && !c.contains(['$', '%']) => {
                stat_abs(ctx, &format!("{}/{}", c.trim_end_matches('/'), prog))
            }
            _ => Located::Unknown,
        };
    }
    if ctx.runner.which(prog).is_some() {
        return Located::Present;
    }
    for d in extra_bin_dirs(ctx) {
        if fs::metadata(d.join(prog)).is_ok() {
            return Located::Present;
        }
    }
    Located::Missing(prog.to_string())
}

fn locate_flatpak(ctx: &Ctx, app: &str) -> Located {
    match locate_program(ctx, "flatpak", None) {
        Located::Present => {}
        other => return other,
    }
    let dirs = [
        ctx.env.sys_path("/var/lib/flatpak/app").join(app),
        ctx.env.user_data_dir.join("flatpak/app").join(app),
    ];
    if dirs.iter().any(|d| d.exists()) {
        return Located::Present;
    }
    // Extra installations (external disks, ...) cannot be searched: do not guess.
    let extra = list_dir(&ctx.env.sys_path("/etc/flatpak/installations.d"));
    if !extra.is_empty() {
        return Located::Unknown;
    }
    Located::Missing(format!("flatpak app {app}"))
}

fn locate_exec(ctx: &Ctx, exec: &str, cwd: Option<&str>) -> Located {
    match desktop::target_of_exec(exec) {
        ExecTarget::Program(p) => locate_program(ctx, &p, cwd),
        ExecTarget::Flatpak(app) => locate_flatpak(ctx, &app),
        ExecTarget::Unknown => Located::Unknown,
    }
}

// ---------------------------------------------------------------- entry point

pub fn scan_category(ctx: &Ctx, id: &str, job: &Job) -> Result<Vec<Found>> {
    job.check_cancelled()?;
    match (ctx.env.os, id) {
        (Os::Linux, "desktop_entries") => launchers(ctx, job, "desktop_entries"),
        (Os::Linux, "autostart") => launchers(ctx, job, "autostart"),
        (Os::Linux | Os::MacOs, "broken_symlinks") => broken_symlinks(ctx, job),
        (Os::Linux, "mime_associations") => mime_associations(ctx),
        (Os::Linux, "user_services") => user_services(ctx, job),
        (Os::Linux, "orphaned_packages") => orphaned_packages(ctx, job),
        (Os::MacOs, "launch_agents") => launch_agents(ctx, job),
        (_, other) => Err(ApiError::invalid_params(format!(
            "unknown category `{other}` for this system"
        ))),
    }
}

// ---------------------------------------------------------------- launchers / autostart

fn read_small(path: &Path) -> Option<String> {
    let m = fs::metadata(path).ok()?;
    if !m.is_file() || m.len() > MAX_FILE_BYTES {
        return None;
    }
    fs::read_to_string(path).ok()
}

fn launchers(ctx: &Ctx, job: &Job, category: &str) -> Result<Vec<Found>> {
    let dirs: Vec<PathBuf> = if category == "autostart" {
        vec![ctx.env.config_dir.join("autostart")]
    } else {
        vec![
            ctx.env.user_data_dir.join("applications"),
            ctx.env.sys_path("/usr/share/applications"),
        ]
    };
    let what = if category == "autostart" {
        "autostart entry"
    } else {
        "launcher"
    };
    let mut out = Vec::new();
    for dir in dirs {
        for path in list_dir(&dir) {
            job.check_cancelled()?;
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            // dangling links are the broken-links category's business
            let Some(content) = read_small(&path) else {
                continue;
            };
            let e = desktop::parse(&content);
            if e.kind.as_deref() != Some("Application") {
                continue;
            }
            let cwd = e.path.as_deref();
            let mut missing = None;
            if let Some(t) = &e.try_exec {
                if let Located::Missing(p) = locate_program(ctx, t.trim().trim_matches('"'), cwd) {
                    missing = Some(p);
                }
            }
            if missing.is_none() {
                if let Some(x) = &e.exec {
                    if let Located::Missing(p) = locate_exec(ctx, x, cwd) {
                        missing = Some(p);
                    }
                }
            }
            let Some(missing) = missing else { continue };
            let system = !is_user_path(ctx, &path);
            let name = e.name.clone().unwrap_or_else(|| {
                path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
            });
            out.push(Found::file(
                category,
                format!("The {what} \"{name}\" starts a program that no longer exists: {missing}"),
                &path,
                Some(missing),
                None,
                if system { Severity::Medium } else { Severity::Low },
                system,
                Action::RemoveFile {
                    path: path.clone(),
                    also: Vec::new(),
                    system,
                },
            ));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- broken symlinks

/// Places whose contents may live on media that is not mounted right now.
const MOUNT_PREFIXES: [&str; 8] = [
    "/mnt/", "/media/", "/run/media/", "/net/", "/Volumes/", "/run/user/", "/var/mnt/", "/sys/",
];

fn under_mount(target: &str) -> bool {
    MOUNT_PREFIXES.iter().any(|p| target.starts_with(p))
        || target == "/mnt"
        || target == "/media"
        || target.starts_with("/proc/")
}

/// `Some(target text)` when `link` is a symlink whose target definitely does not exist.
fn dangling_target(ctx: &Ctx, link: &Path) -> Option<String> {
    let meta = fs::symlink_metadata(link).ok()?;
    if !meta.file_type().is_symlink() {
        return None;
    }
    let target = fs::read_link(link).ok()?;
    let text = target.to_string_lossy().into_owned();
    let (resolved, lexical) = if target.is_absolute() {
        (map_abs(ctx, &text), text.clone())
    } else {
        let joined = link.parent()?.join(&target);
        let lex = crate::safety::normalize(&joined);
        (joined, lex.to_string_lossy().into_owned())
    };
    if under_mount(&lexical) || under_mount(&text) {
        return None;
    }
    match fs::metadata(&resolved) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(text),
        _ => None,
    }
}

fn broken_symlinks(ctx: &Ctx, job: &Job) -> Result<Vec<Found>> {
    let dirs: Vec<PathBuf> = match ctx.env.os {
        Os::MacOs => vec![
            ctx.env.home.join("bin"),
            ctx.env.home.join(".local/bin"),
            ctx.env.sys_path("/usr/local/bin"),
        ],
        _ => vec![
            ctx.env.home.join(".local/bin"),
            ctx.env.config_dir.join("autostart"),
            ctx.env.user_data_dir.join("applications"),
        ],
    };
    let mut out = Vec::new();
    for dir in dirs {
        for path in list_dir(&dir) {
            job.check_cancelled()?;
            let Some(target) = dangling_target(ctx, &path) else {
                continue;
            };
            let system = !is_user_path(ctx, &path);
            out.push(Found::file(
                "broken_symlinks",
                format!(
                    "The link {} points to something that no longer exists: {target}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
                &path,
                None,
                Some(target),
                if system { Severity::Medium } else { Severity::Low },
                system,
                Action::RemoveFile {
                    path: path.clone(),
                    also: Vec::new(),
                    system,
                },
            ));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- mimeapps.list

/// Directories that may hold `.desktop` files.
fn application_dirs(ctx: &Ctx) -> Vec<PathBuf> {
    let mut v = vec![ctx.env.user_data_dir.join("applications")];
    let xdg = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    let mut bases: Vec<PathBuf> = xdg
        .split(':')
        .filter(|s| s.starts_with('/'))
        .map(|s| map_abs(ctx, s))
        .collect();
    for d in [
        "/usr/local/share",
        "/usr/share",
        "/var/lib/flatpak/exports/share",
        "/var/lib/snapd/desktop",
        "/run/current-system/sw/share",
        "/nix/var/nix/profiles/default/share",
    ] {
        bases.push(ctx.env.sys_path(d));
    }
    bases.push(ctx.env.user_data_dir.join("flatpak/exports/share"));
    bases.push(ctx.env.home.join(".nix-profile/share"));
    for b in bases {
        let a = b.join("applications");
        if !v.contains(&a) {
            v.push(a);
        }
    }
    v
}

/// Does the desktop id exist in any application directory? (`a-b.desktop` may be `a/b.desktop`.)
fn desktop_id_exists(dirs: &[PathBuf], id: &str) -> bool {
    let mut cur = id.to_string();
    let mut from = 0;
    loop {
        // never let an id turn into an absolute path
        if cur.starts_with('/') || cur.contains("//") {
            return false;
        }
        if dirs.iter().any(|d| d.join(&cur).exists()) {
            return true;
        }
        let stem_end = cur.len().saturating_sub(".desktop".len());
        let Some(i) = cur
            .get(from..stem_end.max(from))
            .and_then(|s| s.find('-'))
        else {
            return false;
        };
        let at = from + i;
        cur.replace_range(at..at + 1, "/");
        from = at + 1;
    }
}

fn any_desktop_files(dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|d| {
        list_dir(d)
            .iter()
            .any(|p| p.extension().and_then(|e| e.to_str()) == Some("desktop"))
    })
}

fn mime_associations(ctx: &Ctx) -> Result<Vec<Found>> {
    let file = ctx.env.config_dir.join("mimeapps.list");
    // A symlinked list (dotfile managers) is not ours to rewrite.
    match fs::symlink_metadata(&file) {
        Ok(m) if m.file_type().is_file() => {}
        _ => return Ok(Vec::new()),
    }
    let Some(content) = read_small(&file) else {
        return Ok(Vec::new());
    };
    let dirs = application_dirs(ctx);
    // If no application directory has a single launcher we are looking at an unusual
    // environment (minimal container, unknown distribution): conclude nothing.
    if !any_desktop_files(&dirs) {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for a in mimeapps::associations(&content) {
        for id in &a.ids {
            if !id.ends_with(".desktop") || id.starts_with('.') || id.contains(['/', '\0']) {
                continue;
            }
            if desktop_id_exists(&dirs, id) || !seen.insert((a.section.clone(), a.mime.clone(), id.clone())) {
                continue;
            }
            out.push(Found::file(
                "mime_associations",
                format!("\"{}\" is set as an application for {} but is not installed", id, a.mime),
                &file,
                Some(a.mime.clone()),
                Some(format!("{id} ({})", a.section)),
                Severity::Low,
                false,
                Action::MimeRemove {
                    file: file.clone(),
                    section: a.section.clone(),
                    mime: a.mime.clone(),
                    desktop_id: id.clone(),
                },
            ));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- systemd user units

/// The program of one `ExecStart=` value (`-`, `@`, `+`, `!`, `:` prefixes removed).
fn exec_start_program(ctx: &Ctx, value: &str) -> Located {
    let v = value.trim_start_matches(['@', '-', ':', '+', '!']).trim();
    if v.is_empty() || v.ends_with('\\') {
        return Located::Unknown;
    }
    let Some(args) = desktop::split_exec(v) else {
        return Located::Unknown;
    };
    let Some(first) = args.first() else {
        return Located::Unknown;
    };
    // specifiers: only %h (home) and %% are understood
    let mut prog = String::new();
    let mut it = first.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            prog.push(c);
            continue;
        }
        match it.next() {
            Some('h') => prog.push_str(&ctx.env.home.to_string_lossy()),
            Some('%') => prog.push('%'),
            _ => return Located::Unknown,
        }
    }
    if !prog.starts_with('/') {
        return Located::Unknown;
    }
    stat_abs(ctx, &prog)
}

fn user_services(ctx: &Ctx, job: &Job) -> Result<Vec<Found>> {
    let dir = ctx.env.config_dir.join("systemd/user");
    let mut out = Vec::new();
    for path in list_dir(&dir) {
        job.check_cancelled()?;
        let Some(fname) = path.file_name().map(|f| f.to_string_lossy().into_owned()) else {
            continue;
        };
        if !fname.ends_with(".service") {
            continue;
        }
        // real files only: links are usually managed by a dotfile tool
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
            continue;
        }
        // drop-ins may replace ExecStart
        if dir.join(format!("{fname}.d")).exists() {
            continue;
        }
        let Some(content) = read_small(&path) else {
            continue;
        };
        let mut in_service = false;
        let mut starts: Vec<Located> = Vec::new();
        for line in content.lines() {
            let t = line.trim();
            if t.starts_with('[') && t.ends_with(']') {
                in_service = t == "[Service]";
                continue;
            }
            if in_service {
                if let Some(v) = t.strip_prefix("ExecStart=") {
                    if v.trim().is_empty() {
                        starts.clear(); // an empty assignment resets the list
                    } else {
                        starts.push(exec_start_program(ctx, v));
                    }
                }
            }
        }
        if starts.is_empty() || !starts.iter().all(Located::is_missing) {
            continue;
        }
        let Some(Located::Missing(p)) = starts.into_iter().next() else {
            continue;
        };
        // the links that enable the unit
        let mut also = Vec::new();
        for d in list_dir(&dir) {
            let dn = d.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            if !(dn.ends_with(".wants") || dn.ends_with(".requires") || dn.ends_with(".upholds")) {
                continue;
            }
            let link = d.join(&fname);
            if let Ok(m) = fs::symlink_metadata(&link) {
                if m.file_type().is_symlink()
                    && fs::read_link(&link)
                        .ok()
                        .and_then(|t| t.file_name().map(|f| f.to_string_lossy() == fname))
                        == Some(true)
                {
                    also.push(link);
                }
            }
        }
        out.push(Found::file(
            "user_services",
            format!("The user service {fname} starts a program that no longer exists: {p}"),
            &path,
            Some(p),
            None,
            Severity::Low,
            false,
            Action::RemoveFile {
                path: path.clone(),
                also,
                system: false,
            },
        ));
    }
    Ok(out)
}

// ---------------------------------------------------------------- orphaned packages

/// `Remv libfoo:amd64 [1.2-3] (...)` -> `("libfoo:amd64", "1.2-3")`.
pub fn parse_apt_autoremove(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("Remv ")?;
            let mut it = rest.split_whitespace();
            let name = it.next()?;
            let ver = it
                .next()
                .and_then(|v| v.strip_prefix('[')?.strip_suffix(']'))
                .unwrap_or("");
            Some((name.to_string(), ver.to_string()))
        })
        .collect()
}

/// `name evr` per line (from `--queryformat "%{name} %{evr}\n"`).
pub fn parse_dnf_unneeded(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (n, v) = (it.next()?, it.next()?);
            it.next().is_none().then(|| (n.to_string(), v.to_string()))
        })
        .collect()
}

/// One package name per line (`pacman -Qdtq`).
pub fn parse_pacman_orphans(out: &str) -> Vec<(String, String)> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.contains(char::is_whitespace))
        .map(|l| (l.to_string(), String::new()))
        .collect()
}

pub fn detect_manager(ctx: &Ctx) -> Option<PkgManager> {
    if ctx.runner.which("apt-get").is_some() {
        Some(PkgManager::Apt)
    } else if ctx.runner.which("dnf").is_some() {
        Some(PkgManager::Dnf)
    } else if ctx.runner.which("pacman").is_some() {
        Some(PkgManager::Pacman)
    } else {
        None
    }
}

fn orphaned_packages(ctx: &Ctx, job: &Job) -> Result<Vec<Found>> {
    let Some(mgr) = detect_manager(ctx) else {
        return Ok(Vec::new());
    };
    job.check_cancelled()?;
    let list = match mgr {
        PkgManager::Apt => ctx
            .runner
            .run("apt-get", &["-s", "autoremove"])
            .ok()
            .filter(|o| o.success())
            .map(|o| parse_apt_autoremove(&o.stdout)),
        PkgManager::Dnf => ctx
            .runner
            .run(
                "dnf",
                &["repoquery", "--unneeded", "--queryformat", "%{name} %{evr}\\n"],
            )
            .ok()
            .filter(|o| o.success())
            .map(|o| parse_dnf_unneeded(&o.stdout)),
        PkgManager::Pacman => ctx
            .runner
            .run("pacman", &["-Qdtq"])
            .ok()
            // exit status 1 with no output means "no orphans"
            .map(|o| parse_pacman_orphans(&o.stdout)),
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (name, ver) in list.unwrap_or_default() {
        if !valid_pkg_name(&name) || !seen.insert(name.clone()) {
            continue;
        }
        if out.len() >= MAX_PACKAGES {
            break;
        }
        out.push(Found::file(
            "orphaned_packages",
            format!("The package {name} was installed as a dependency and nothing needs it any more"),
            Path::new(mgr.name()),
            Some(name.clone()),
            Some(ver.clone()).filter(|v| !v.is_empty()),
            Severity::Medium,
            true,
            Action::RemovePackage {
                manager: mgr,
                package: name,
                version: ver,
            },
        ));
    }
    Ok(out)
}

// ---------------------------------------------------------------- macOS launch agents

fn plist_program(v: &plist::Value) -> Option<Option<String>> {
    let d = v.as_dictionary()?;
    // App-bundle based agents (SMAppService) have no path to judge.
    if d.contains_key("BundleProgram") {
        return Some(None);
    }
    if let Some(p) = d.get("Program").and_then(|p| p.as_string()) {
        return Some(Some(p.to_string()));
    }
    let first = d
        .get("ProgramArguments")
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|s| s.as_string())?;
    Some(Some(first.to_string()))
}

fn launch_agents(ctx: &Ctx, job: &Job) -> Result<Vec<Found>> {
    let dir = ctx.env.home.join("Library/LaunchAgents");
    let mut out = Vec::new();
    for path in list_dir(&dir) {
        job.check_cancelled()?;
        if path.extension().and_then(|e| e.to_str()) != Some("plist") {
            continue;
        }
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
            continue;
        }
        let Ok(v) = plist::Value::from_file(&path) else {
            continue;
        };
        let Some(Some(prog)) = plist_program(&v) else {
            continue;
        };
        // launchd does not expand `~`, variables or relative paths: only absolute ones are judged
        if !prog.starts_with('/') || prog.starts_with("/Volumes/") {
            continue;
        }
        let Located::Missing(p) = stat_abs(ctx, &prog) else {
            continue;
        };
        let label = v
            .as_dictionary()
            .and_then(|d| d.get("Label"))
            .and_then(|l| l.as_string())
            .map(str::to_string)
            .unwrap_or_else(|| path.file_name().unwrap_or_default().to_string_lossy().into_owned());
        out.push(Found::file(
            "launch_agents",
            format!("The launch agent {label} starts a program that no longer exists: {p}"),
            &path,
            Some(p),
            None,
            Severity::Low,
            false,
            Action::RemoveFile {
                path: path.clone(),
                also: Vec::new(),
                system: false,
            },
        ));
    }
    Ok(out)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::runner::{CmdOutput, MockRunner};
    use std::os::unix::fs::symlink;

    struct T {
        _d: tempfile::TempDir,
        ctx: Ctx,
        mock: MockRunner,
    }

    fn t() -> T {
        let d = tempfile::tempdir().unwrap();
        let mock = MockRunner::new();
        let ctx = Ctx::test(d.path(), mock.clone());
        fs::create_dir_all(&ctx.env.home).unwrap();
        fs::create_dir_all(&ctx.env.root).unwrap();
        T { _d: d, ctx, mock }
    }

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    fn scan(t: &T, id: &str) -> Vec<Found> {
        scan_category(&t.ctx, id, &Job::detached()).unwrap()
    }

    fn user_apps(t: &T) -> PathBuf {
        t.ctx.env.user_data_dir.join("applications")
    }

    fn desktop_file(exec: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName=Thing\nExec={exec}\n")
    }

    #[test]
    fn launchers_valid_and_broken() {
        let t = t();
        let bin = t.ctx.env.sys_path("/usr/bin");
        write(&bin.join("present"), "#!/bin/sh\n");
        write(&t.ctx.env.sys_path("/bin/sh"), "");
        write(&t.ctx.env.home.join("tools/mine"), "#!/bin/sh\n");
        t.mock.with_program("onpath");
        let apps = user_apps(&t);
        write(&apps.join("ok-abs.desktop"), &desktop_file("/usr/bin/present %U"));
        write(&apps.join("ok-bare.desktop"), &desktop_file("present --x"));
        write(&apps.join("ok-path.desktop"), &desktop_file("onpath"));
        write(&apps.join("ok-quoted.desktop"), &desktop_file(&format!("\"{}/tools/mine\" --a %F", t.ctx.env.home.display())));
        write(&apps.join("ok-env.desktop"), &desktop_file("env FOO=1 /usr/bin/present"));
        write(&apps.join("ok-var.desktop"), &desktop_file("$HOME/bin/x")); // unknown: not reported
        write(&apps.join("ok-sh.desktop"), &desktop_file("sh -c \"gone-thing\"")); // sh is not judged by its script
        write(&apps.join("ok-noexec.desktop"), "[Desktop Entry]\nType=Application\nName=X\nDBusActivatable=true\n");
        write(&apps.join("ok-link.desktop"), "[Desktop Entry]\nType=Link\nName=X\nURL=http://x\n");
        write(&apps.join("ok-notdesktop.txt"), &desktop_file("/nowhere/x"));
        write(&apps.join("bad-abs.desktop"), &desktop_file("/opt/gone/app %U"));
        write(&apps.join("bad-bare.desktop"), &desktop_file("definitely-not-installed --flag"));
        write(&apps.join("bad-quoted.desktop"), &desktop_file("\"/opt/My Gone App/bin/app\" %F"));
        write(&apps.join("bad-env.desktop"), &desktop_file("env A=b /opt/gone/envprog"));
        write(
            &apps.join("bad-tryexec.desktop"),
            "[Desktop Entry]\nType=Application\nName=Try\nTryExec=/opt/gone/try\nExec=/usr/bin/present\n",
        );
        write(&apps.join("ok-relpath.desktop"), "[Desktop Entry]\nType=Application\nName=R\nPath=/usr/bin\nExec=./present\n");
        write(&apps.join("bad-relpath.desktop"), "[Desktop Entry]\nType=Application\nName=R\nPath=/usr/bin\nExec=./gone\n");
        let f = scan(&t, "desktop_entries");
        let mut names: Vec<_> = f
            .iter()
            .map(|x| Path::new(&x.issue.location).file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "bad-abs.desktop",
                "bad-bare.desktop",
                "bad-env.desktop",
                "bad-quoted.desktop",
                "bad-relpath.desktop",
                "bad-tryexec.desktop"
            ]
        );
        let q = f.iter().find(|x| x.issue.location.ends_with("bad-quoted.desktop")).unwrap();
        assert_eq!(q.issue.value.as_deref(), Some("/opt/My Gone App/bin/app"));
        assert!(!q.issue.needs_admin);
        assert_eq!(q.issue.severity, Severity::Low);
        assert!(matches!(&q.action, Action::RemoveFile { system: false, .. }));
    }

    #[test]
    fn system_launchers_are_medium_and_need_admin() {
        let t = t();
        let sys = t.ctx.env.sys_path("/usr/share/applications");
        write(&sys.join("gone.desktop"), &desktop_file("/opt/gone/app"));
        let f = scan(&t, "desktop_entries");
        assert_eq!(f.len(), 1);
        assert!(f[0].issue.needs_admin);
        assert_eq!(f[0].issue.severity, Severity::Medium);
        assert!(matches!(&f[0].action, Action::RemoveFile { system: true, .. }));
    }

    #[test]
    fn flatpak_launchers() {
        let t = t();
        let apps = user_apps(&t);
        write(&apps.join("gimp.desktop"), &desktop_file("flatpak run --branch=stable org.gimp.GIMP @@u %U @@"));
        // no flatpak at all -> the launcher cannot work
        let f = scan(&t, "desktop_entries");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].issue.value.as_deref(), Some("flatpak"));
        // flatpak installed, app not -> broken
        t.mock.with_program("flatpak");
        let f = scan(&t, "desktop_entries");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].issue.value.as_deref(), Some("flatpak app org.gimp.GIMP"));
        // app installed (system)
        fs::create_dir_all(t.ctx.env.sys_path("/var/lib/flatpak/app/org.gimp.GIMP")).unwrap();
        assert!(scan(&t, "desktop_entries").is_empty());
        // app removed again but extra installations exist -> unknown
        fs::remove_dir_all(t.ctx.env.sys_path("/var/lib/flatpak/app/org.gimp.GIMP")).unwrap();
        write(&t.ctx.env.sys_path("/etc/flatpak/installations.d/usb.conf"), "[Installation \"usb\"]\n");
        assert!(scan(&t, "desktop_entries").is_empty());
        // user installation
        fs::remove_file(t.ctx.env.sys_path("/etc/flatpak/installations.d/usb.conf")).unwrap();
        fs::create_dir_all(t.ctx.env.user_data_dir.join("flatpak/app/org.gimp.GIMP")).unwrap();
        assert!(scan(&t, "desktop_entries").is_empty());
    }

    #[test]
    fn autostart_entries() {
        let t = t();
        let dir = t.ctx.env.config_dir.join("autostart");
        write(&dir.join("gone.desktop"), "[Desktop Entry]\nType=Application\nName=Gone\nExec=/opt/gone/x\nHidden=true\n");
        write(&dir.join("gone2.desktop"), "[Desktop Entry]\nType=Application\nName=Gone2\nExec=/opt/gone/y\nX-GNOME-Autostart-enabled=false\n");
        write(&t.ctx.env.sys_path("/usr/bin/fine"), "");
        write(&dir.join("ok.desktop"), &desktop_file("/usr/bin/fine"));
        let f = scan(&t, "autostart");
        assert_eq!(f.len(), 2);
        assert!(f.iter().all(|x| x.issue.category == "autostart"));
        // launchers scanning does not look at autostart
        assert!(scan(&t, "desktop_entries").is_empty());
    }

    #[test]
    fn broken_symlinks() {
        let t = t();
        let bin = t.ctx.env.home.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        write(&t.ctx.env.home.join("real/tool"), "x");
        symlink(t.ctx.env.home.join("real/tool"), bin.join("good-abs")).unwrap();
        symlink("../../real/tool", bin.join("good-rel")).unwrap();
        symlink(t.ctx.env.home.join("real/gone"), bin.join("bad-abs")).unwrap();
        symlink("../../real/gone", bin.join("bad-rel")).unwrap();
        symlink("/opt/definitely/gone", bin.join("bad-sys")).unwrap();
        symlink("/mnt/nas/tools/x", bin.join("nas")).unwrap(); // may just be unmounted
        symlink("/media/usb/x", bin.join("usb")).unwrap();
        symlink("loop", bin.join("loop")).unwrap(); // ELOOP: not "not found"
        write(&bin.join("regular"), "x");
        let apps = user_apps(&t);
        fs::create_dir_all(&apps).unwrap();
        symlink("/opt/gone/app.desktop", apps.join("dangling.desktop")).unwrap();
        let f = scan(&t, "broken_symlinks");
        let mut names: Vec<_> = f
            .iter()
            .map(|x| Path::new(&x.issue.location).file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["bad-abs", "bad-rel", "bad-sys", "dangling.desktop"]);
        let s = f.iter().find(|x| x.issue.location.ends_with("bad-sys")).unwrap();
        assert_eq!(s.issue.data.as_deref(), Some("/opt/definitely/gone"));
        // a dangling launcher link is not double-reported as a broken launcher
        assert!(scan(&t, "desktop_entries").is_empty());
    }

    #[test]
    fn absolute_link_targets_follow_the_redirected_root() {
        let t = t();
        let bin = t.ctx.env.home.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        // /usr/bin/present exists only under the sandbox root
        write(&t.ctx.env.sys_path("/usr/bin/present"), "x");
        symlink("/usr/bin/present", bin.join("ok")).unwrap();
        assert!(scan(&t, "broken_symlinks").is_empty());
    }

    #[test]
    fn mime_associations() {
        let t = t();
        write(&t.ctx.env.sys_path("/usr/share/applications/gedit.desktop"), "[Desktop Entry]\n");
        write(&t.ctx.env.sys_path("/usr/share/applications/org.gnome.Eog.desktop"), "[Desktop Entry]\n");
        write(&t.ctx.env.sys_path("/usr/share/applications/kde/dolphin.desktop"), "[Desktop Entry]\n");
        write(&user_apps(&t).join("mine.desktop"), "[Desktop Entry]\n");
        let list = t.ctx.env.config_dir.join("mimeapps.list");
        let body = "[Default Applications]\ntext/plain=gedit.desktop;gone1.desktop;\nimage/png=org.gnome.Eog.desktop\ninode/directory=kde-dolphin.desktop;\nx/y=mine.desktop;\n\n[Added Associations]\ntext/html=gone2.desktop;gedit.desktop;\n\n[Removed Associations]\ntext/x=gone3.desktop;\n";
        write(&list, body);
        let f = scan(&t, "mime_associations");
        let mut ids: Vec<_> = f
            .iter()
            .map(|x| x.issue.data.clone().unwrap())
            .collect();
        ids.sort();
        assert_eq!(ids, ["gone1.desktop (Default Applications)", "gone2.desktop (Added Associations)"]);
        assert!(matches!(&f[0].action, Action::MimeRemove { .. }));
        // untouched by scanning
        assert_eq!(fs::read_to_string(&list).unwrap(), body);
    }

    #[test]
    fn mime_scan_is_skipped_in_odd_environments() {
        let t = t();
        let list = t.ctx.env.config_dir.join("mimeapps.list");
        write(&list, "[Default Applications]\ntext/plain=gedit.desktop;\n");
        // no application directory has any launcher at all
        assert!(scan(&t, "mime_associations").is_empty());
        // a symlinked list is not touched
        write(&t.ctx.env.sys_path("/usr/share/applications/other.desktop"), "[Desktop Entry]\n");
        assert_eq!(scan(&t, "mime_associations").len(), 1);
        fs::remove_file(&list).unwrap();
        write(&t.ctx.env.home.join("dotfiles/mimeapps.list"), "[Default Applications]\ntext/plain=gedit.desktop;\n");
        symlink(t.ctx.env.home.join("dotfiles/mimeapps.list"), &list).unwrap();
        assert!(scan(&t, "mime_associations").is_empty());
    }

    #[test]
    fn desktop_id_lookup_with_dashes() {
        let d = tempfile::tempdir().unwrap();
        write(&d.path().join("a/b-c.desktop"), "");
        write(&d.path().join("x.desktop"), "");
        let dirs = vec![d.path().to_path_buf()];
        assert!(desktop_id_exists(&dirs, "x.desktop"));
        assert!(desktop_id_exists(&dirs, "a-b-c.desktop"));
        assert!(!desktop_id_exists(&dirs, "a-b-d.desktop"));
        assert!(!desktop_id_exists(&dirs, "nope.desktop"));
        assert!(!desktop_id_exists(&dirs, "-.desktop"));
    }

    #[test]
    fn user_services() {
        let t = t();
        let dir = t.ctx.env.config_dir.join("systemd/user");
        write(&t.ctx.env.sys_path("/usr/bin/fine"), "");
        write(&dir.join("bad.service"), "[Unit]\nDescription=x\n[Service]\nExecStart=/opt/gone/daemon --x\n[Install]\nWantedBy=default.target\n");
        write(&dir.join("good.service"), "[Service]\nExecStart=/usr/bin/fine\n");
        write(&dir.join("dash.service"), "[Service]\nExecStart=-/opt/gone/dash\n");
        write(&dir.join("home.service"), "[Service]\nExecStart=%h/bin/gone\n");
        write(&dir.join("spec.service"), "[Service]\nExecStart=/opt/%N/gone\n"); // unknown specifier
        write(&dir.join("bare.service"), "[Service]\nExecStart=gone\n"); // not absolute: invalid, not judged
        write(&dir.join("multi.service"), "[Service]\nExecStart=/opt/gone/a\nExecStart=/usr/bin/fine\n");
        write(&dir.join("reset.service"), "[Service]\nExecStart=/opt/gone/a\nExecStart=\nExecStart=/usr/bin/fine\n");
        write(&dir.join("dropin.service"), "[Service]\nExecStart=/opt/gone/a\n");
        write(&dir.join("dropin.service.d/override.conf"), "[Service]\nExecStart=\nExecStart=/usr/bin/fine\n");
        write(&dir.join("noexec.service"), "[Service]\nType=oneshot\n");
        write(&dir.join("other.timer"), "[Timer]\n");
        symlink("/opt/gone/x", dir.join("linked.service")).unwrap();
        fs::create_dir_all(dir.join("default.target.wants")).unwrap();
        symlink("../bad.service", dir.join("default.target.wants/bad.service")).unwrap();
        symlink("/usr/lib/systemd/user/other.service", dir.join("default.target.wants/other.service")).unwrap();
        let f = scan(&t, "user_services");
        let mut names: Vec<_> = f
            .iter()
            .map(|x| Path::new(&x.issue.location).file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["bad.service", "dash.service", "home.service"]);
        let bad = f.iter().find(|x| x.issue.location.ends_with("bad.service")).unwrap();
        match &bad.action {
            Action::RemoveFile { also, system, .. } => {
                assert!(!system);
                assert_eq!(also.len(), 1);
                assert!(also[0].ends_with("default.target.wants/bad.service"));
            }
            a => panic!("{a:?}"),
        }
    }

    #[test]
    fn package_output_parsers() {
        let apt = "Reading package lists...\nBuilding dependency tree...\nThe following packages will be REMOVED:\n  libfoo libbar:amd64\nRemv libfoo [1.2-3]\nRemv libbar:amd64 [2.0] [ubuntu:noble]\nPurg junk [1]\n";
        assert_eq!(
            parse_apt_autoremove(apt),
            [("libfoo".to_string(), "1.2-3".to_string()), ("libbar:amd64".to_string(), "2.0".to_string())]
        );
        assert!(parse_apt_autoremove("0 upgraded, 0 newly installed\n").is_empty());
        let dnf = "Last metadata expiration check: 0:10:00 ago on Mon.\nlibfoo 1.2-3.fc40\nlibbar 2:4.5-1.fc40\n";
        assert_eq!(
            parse_dnf_unneeded(dnf),
            [("libfoo".to_string(), "1.2-3.fc40".to_string()), ("libbar".to_string(), "2:4.5-1.fc40".to_string())]
        );
        assert_eq!(
            parse_pacman_orphans("foo\nbar-git\n\nwarning: something odd here\n"),
            [("foo".to_string(), String::new()), ("bar-git".to_string(), String::new())]
        );
    }

    #[test]
    fn orphaned_packages_per_manager() {
        let t = t();
        t.mock.on(
            "apt-get",
            &["-s", "autoremove"],
            CmdOutput::ok("Remv libfoo [1.2-3]\nRemv -evil [1]\nRemv libbar [2.0]\nRemv libfoo [1.2-3]\n"),
        );
        let f = scan(&t, "orphaned_packages");
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].issue.value.as_deref(), Some("libfoo"));
        assert_eq!(f[0].issue.data.as_deref(), Some("1.2-3"));
        assert_eq!(f[0].issue.location, "apt");
        assert!(f[0].issue.needs_admin);
        assert_eq!(f[0].issue.severity, Severity::Medium);

        let t = self::t();
        t.mock.on("dnf", &["repoquery", "--unneeded", "--queryformat", "%{name} %{evr}\\n"], CmdOutput::ok("libx 1-1\n"));
        assert_eq!(scan(&t, "orphaned_packages").len(), 1);

        let t = self::t();
        t.mock.on("pacman", &["-Qdtq"], CmdOutput::failed(1, ""));
        assert!(scan(&t, "orphaned_packages").is_empty());
        let t = self::t();
        t.mock.on("pacman", &["-Qdtq"], CmdOutput::ok("orphan-a\n"));
        assert_eq!(scan(&t, "orphaned_packages").len(), 1);

        // no package manager, or a failing one
        let t = self::t();
        assert!(scan(&t, "orphaned_packages").is_empty());
        let t = self::t();
        t.mock.on("apt-get", &["-s", "autoremove"], CmdOutput::failed(100, "E: no"));
        assert!(scan(&t, "orphaned_packages").is_empty());
    }

    #[test]
    fn launch_agents_on_macos() {
        let d = tempfile::tempdir().unwrap();
        let mut ctx = Ctx::test(d.path(), MockRunner::new());
        ctx.env.os = Os::MacOs;
        let dir = ctx.env.home.join("Library/LaunchAgents");
        fs::create_dir_all(&dir).unwrap();
        write(&ctx.env.sys_path("/usr/bin/true"), "");
        let plist = |label: &str, body: &str| {
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{label}</string>{body}</dict></plist>\n")
        };
        write(&dir.join("com.gone.a.plist"), &plist("com.gone.a", "<key>ProgramArguments</key><array><string>/Applications/Gone.app/Contents/MacOS/gone</string><string>-x</string></array>"));
        write(&dir.join("com.gone.b.plist"), &plist("com.gone.b", "<key>Program</key><string>/opt/gone/b</string>"));
        write(&dir.join("com.ok.plist"), &plist("com.ok", "<key>ProgramArguments</key><array><string>/usr/bin/true</string></array>"));
        write(&dir.join("com.bare.plist"), &plist("com.bare", "<key>ProgramArguments</key><array><string>gone</string></array>"));
        write(&dir.join("com.tilde.plist"), &plist("com.tilde", "<key>Program</key><string>~/gone</string>"));
        write(&dir.join("com.vol.plist"), &plist("com.vol", "<key>Program</key><string>/Volumes/Ext/gone</string>"));
        write(&dir.join("com.bundle.plist"), &plist("com.bundle", "<key>BundleProgram</key><string>Contents/MacOS/gone</string>"));
        write(&dir.join("com.broken.plist"), "not a plist");
        write(&dir.join("readme.txt"), "x");
        let f = scan_category(&ctx, "launch_agents", &Job::detached()).unwrap();
        let mut names: Vec<_> = f
            .iter()
            .map(|x| Path::new(&x.issue.location).file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["com.gone.a.plist", "com.gone.b.plist"]);
        // macOS broken links include /usr/local/bin (needs admin)
        let lb = ctx.env.sys_path("/usr/local/bin");
        fs::create_dir_all(&lb).unwrap();
        symlink("../Cellar/gone/1.0/bin/gone", lb.join("gone")).unwrap();
        let f = scan_category(&ctx, "broken_symlinks", &Job::detached()).unwrap();
        assert_eq!(f.len(), 1);
        assert!(f[0].issue.needs_admin);
        // categories are per OS
        assert!(scan_category(&ctx, "desktop_entries", &Job::detached()).is_err());
    }

    #[test]
    fn unknown_or_foreign_category_is_rejected() {
        let t = t();
        assert!(scan_category(&t.ctx, "nope", &Job::detached()).is_err());
        assert!(scan_category(&t.ctx, "launch_agents", &Job::detached()).is_err());
    }

    #[test]
    fn locate_program_rules() {
        let t = t();
        assert_eq!(locate_program(&t.ctx, "", None), Located::Unknown);
        assert_eq!(locate_program(&t.ctx, "$HOME/x", None), Located::Unknown);
        assert_eq!(locate_program(&t.ctx, "~/x", None), Located::Unknown);
        assert_eq!(locate_program(&t.ctx, "./x", None), Located::Unknown);
        assert_eq!(locate_program(&t.ctx, "sub/x", Some("relative")), Located::Unknown);
        assert_eq!(
            locate_program(&t.ctx, "/opt/gone", None),
            Located::Missing("/opt/gone".into())
        );
        assert_eq!(
            locate_program(&t.ctx, "nothing-here", None),
            Located::Missing("nothing-here".into())
        );
        // found in a user bin dir that is not on this process's PATH
        write(&t.ctx.env.home.join(".cargo/bin/mytool"), "");
        assert_eq!(locate_program(&t.ctx, "mytool", None), Located::Present);
    }
}
