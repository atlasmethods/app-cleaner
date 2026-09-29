//! Leftover detection after an uninstall, and their (server-verified) removal.
//!
//! Matching is deliberately conservative: a folder is a candidate only when its name is
//! exactly (case-insensitive) one of the names derived from the application's display
//! name, package id or bundle id, inside a per-user application-data root. Protected
//! paths, OS trees and the user's exclusions are never candidates.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use walkdir::WalkDir;

use crate::ctx::{Ctx, Os};
use crate::error::Result;
use crate::features::settings;
use crate::features::uninstall::model::Leftover;
use crate::job::Job;
use crate::pkgutil::path_size;
use crate::safety::{io_message, ExcludeSet, SafeDeleter, Safety};

/// Folder names shared by many programs; never treated as belonging to one application.
const GENERIC: &[&str] = &[
    "app",
    "apps",
    "application",
    "applications",
    "autostart",
    "backups",
    "bin",
    "cache",
    "caches",
    "common",
    "config",
    "crashreports",
    "crash reports",
    "data",
    "default",
    "desktop",
    "dconf",
    "documents",
    "downloads",
    "flatpak",
    "fontconfig",
    "gnome",
    "gnupg",
    "icons",
    "java",
    "kde",
    "lib",
    "library",
    "local",
    "log",
    "logs",
    "microsoft",
    "mime",
    "nautilus",
    "node",
    "npm",
    "nvidia",
    "packages",
    "pip",
    "pipewire",
    "programs",
    "pulse",
    "python",
    "settings",
    "share",
    "snap",
    "ssh",
    "system",
    "systemd",
    "temp",
    "themes",
    "tmp",
    "trash",
    "user",
    "users",
    "windows",
    "apple",
    "google",
    "intel",
    "amd",
    "adobe",
    "xdg",
    "gtk-2.0",
    "gtk-3.0",
    "gtk-4.0",
];

/// Reverse-DNS bundle / flatpak id charset.
pub fn valid_bundle_id(s: &str) -> bool {
    s.len() >= 5
        && s.len() <= 200
        && s.contains('.')
        && s.split('.').all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        && !s.to_lowercase().starts_with("com.apple.")
}

fn strip_parenthetical(name: &str) -> String {
    let mut out = String::new();
    let mut depth = 0u32;
    for c in name.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Lower-case folder names that could belong to the application.
pub fn candidate_names(name: &str, id: &str) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    let base = strip_parenthetical(name).to_lowercase();
    if !base.is_empty() {
        raw.push(base.clone());
        raw.push(base.replace(' ', "-"));
        raw.push(base.replace(' ', "_"));
        raw.push(base.replace(' ', ""));
    }
    if let Some(("dpkg" | "rpm" | "pacman" | "snap" | "brew" | "brew-cask", key)) =
        id.split_once(':')
    {
        raw.push(key.to_lowercase());
    }
    let mut seen = HashSet::new();
    raw.into_iter()
        .filter(|n| n.chars().count() >= 3)
        .filter(|n| !n.starts_with('.') && !n.contains(['/', '\\']) && !n.contains(".."))
        .filter(|n| !GENERIC.contains(&n.as_str()))
        .filter(|n| seen.insert(n.clone()))
        .collect()
}

fn find_child(parent: &Path, wanted: &HashSet<String>) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(parent) else {
        return Vec::new();
    };
    rd.filter_map(|e| e.ok())
        .filter(|e| wanted.contains(&e.file_name().to_string_lossy().to_lowercase()))
        .map(|e| e.path())
        .collect()
}

/// First word of a `.desktop` `Exec=` value, with quotes removed.
fn exec_program(value: &str) -> Option<String> {
    let v = value.trim();
    if let Some(rest) = v.strip_prefix('"') {
        return rest.split('"').next().map(str::to_string);
    }
    v.split_whitespace().next().map(str::to_string)
}

fn desktop_matches(
    content: &str,
    stem: &str,
    names: &HashSet<String>,
    exec_path: Option<&Path>,
) -> bool {
    if names.contains(&stem.to_lowercase()) {
        return true;
    }
    for line in content.lines() {
        let Some(val) = line
            .strip_prefix("Exec=")
            .or_else(|| line.strip_prefix("TryExec="))
        else {
            continue;
        };
        let Some(prog) = exec_program(val) else {
            continue;
        };
        if let Some(p) = exec_path {
            if Path::new(&prog) == p {
                return true;
            }
        }
        let base = prog
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&prog)
            .to_lowercase();
        if names.contains(&base) {
            return true;
        }
    }
    false
}

/// `.desktop` launchers in the user's applications folder that start this application.
pub fn desktop_entries(
    ctx: &Ctx,
    names: &HashSet<String>,
    exec_path: Option<&Path>,
) -> Vec<PathBuf> {
    let dir = ctx.env.user_data_dir.join("applications");
    let Ok(rd) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.filter_map(|e| e.ok()) {
        let fname = e.file_name().to_string_lossy().into_owned();
        let Some(stem) = fname.strip_suffix(".desktop") else {
            continue;
        };
        let Ok(meta) = fs::symlink_metadata(e.path()) else {
            continue;
        };
        if !meta.file_type().is_file() || meta.len() > 256 * 1024 {
            continue;
        }
        let Ok(content) = fs::read_to_string(e.path()) else {
            continue;
        };
        if desktop_matches(&content, stem, names, exec_path) {
            out.push(e.path());
        }
    }
    out
}

/// Candidate leftovers for an application (`name`, `id`, optional macOS `bundle_id`).
pub fn scan(ctx: &Ctx, name: &str, id: &str, bundle_id: Option<&str>) -> Vec<Leftover> {
    let names = candidate_names(name, id);
    let set: HashSet<String> = names.iter().cloned().collect();
    let bundle = bundle_id.filter(|b| valid_bundle_id(b)).map(str::to_string);
    let flatpak_id = id
        .strip_prefix("flatpak:")
        .filter(|f| valid_bundle_id(f))
        .map(str::to_string);
    let snap_name = id.strip_prefix("snap:").map(str::to_string);

    let mut hits: Vec<(PathBuf, &'static str)> = Vec::new();
    let home = &ctx.env.home;
    match ctx.env.os {
        Os::Linux => {
            hits.extend(
                find_child(&ctx.env.config_dir, &set)
                    .into_iter()
                    .map(|p| (p, "config")),
            );
            hits.extend(
                find_child(&ctx.env.cache_dir, &set)
                    .into_iter()
                    .map(|p| (p, "cache")),
            );
            let mut data_roots = vec![ctx.env.user_data_dir.clone()];
            if ctx.env.data_local_dir != ctx.env.user_data_dir {
                data_roots.push(ctx.env.data_local_dir.clone());
            }
            for r in &data_roots {
                hits.extend(find_child(r, &set).into_iter().map(|p| (p, "data")));
            }
            let dot: HashSet<String> = set.iter().map(|n| format!(".{n}")).collect();
            hits.extend(find_child(home, &dot).into_iter().map(|p| (p, "config")));
            if let Some(f) = &flatpak_id {
                let p = home.join(".var").join("app").join(f);
                if p.is_dir() {
                    hits.push((p, "data"));
                }
            }
            if let Some(s) = &snap_name {
                if crate::pkgutil::valid_pkg_name(s) {
                    let p = home.join("snap").join(s);
                    if p.is_dir() {
                        hits.push((p, "data"));
                    }
                }
            }
            let exec_path = crate::features::uninstall::linux::appimage_path_of(id);
            hits.extend(
                desktop_entries(ctx, &set, exec_path)
                    .into_iter()
                    .map(|p| (p, "launcher")),
            );
        }
        Os::Windows => {
            hits.extend(
                find_child(&ctx.env.user_data_dir, &set)
                    .into_iter()
                    .map(|p| (p, "data")),
            );
            hits.extend(
                find_child(&ctx.env.data_local_dir, &set)
                    .into_iter()
                    .map(|p| (p, "data")),
            );
            hits.extend(
                find_child(&ctx.env.sys_path("/ProgramData"), &set)
                    .into_iter()
                    .map(|p| (p, "data")),
            );
        }
        Os::MacOs => {
            let lib = home.join("Library");
            hits.extend(
                find_child(&lib.join("Application Support"), &set)
                    .into_iter()
                    .map(|p| (p, "data")),
            );
            hits.extend(
                find_child(&lib.join("Caches"), &set)
                    .into_iter()
                    .map(|p| (p, "cache")),
            );
            hits.extend(
                find_child(&lib.join("Logs"), &set)
                    .into_iter()
                    .map(|p| (p, "logs")),
            );
            if let Some(b) = &bundle {
                let lb = b.to_lowercase();
                let one = |s: String| HashSet::from([s]);
                hits.extend(
                    find_child(&lib.join("Preferences"), &one(format!("{lb}.plist")))
                        .into_iter()
                        .map(|p| (p, "prefs")),
                );
                hits.extend(
                    find_child(&lib.join("Caches"), &one(lb.clone()))
                        .into_iter()
                        .map(|p| (p, "cache")),
                );
                hits.extend(
                    find_child(&lib.join("Application Support"), &one(lb.clone()))
                        .into_iter()
                        .map(|p| (p, "data")),
                );
                hits.extend(
                    find_child(&lib.join("Containers"), &one(lb.clone()))
                        .into_iter()
                        .map(|p| (p, "data")),
                );
                hits.extend(
                    find_child(&lib.join("HTTPStorages"), &one(lb.clone()))
                        .into_iter()
                        .map(|p| (p, "cache")),
                );
                hits.extend(
                    find_child(
                        &lib.join("Saved Application State"),
                        &one(format!("{lb}.savedstate")),
                    )
                    .into_iter()
                    .map(|p| (p, "cache")),
                );
            }
        }
    }

    let protected = crate::safety::Protected::new(&ctx.env);
    let excludes = ExcludeSet::from_settings(&ctx.env, &settings::load(ctx)).ok();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (p, kind) in hits {
        let Ok(meta) = fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if protected.is_protected(&p) || protected.in_system_tree(&p) {
            continue;
        }
        match &excludes {
            Some(x) if x.is_excluded(&p) => continue,
            None => continue, // unreadable exclusion list: refuse to propose deletions
            _ => {}
        }
        if !seen.insert(p.clone()) {
            continue;
        }
        out.push(Leftover {
            path: p.to_string_lossy().into_owned(),
            size_bytes: if meta.is_dir() {
                path_size(&p)
            } else {
                meta.len()
            },
            kind: kind.to_string(),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathResult {
    pub path: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub bytes: u64,
}

fn delete_one(safety: &Arc<Safety>, path: &Path, job: &Job) -> std::result::Result<u64, String> {
    // The tree walk below checks every entry, but a protected folder itself must be refused
    // before its (unprotected) contents are touched.
    let path = crate::safety::normalize(path);
    let path = path.as_path();
    if !path.is_absolute() {
        return Err("path must be absolute".into());
    }
    if safety.protected.is_protected(path) || safety.protected.in_system_tree(path) {
        return Err("refused: path is protected".into());
    }
    let parent = path.parent().ok_or("path has no parent")?;
    let deleter = SafeDeleter::for_selection(safety.clone(), parent).map_err(|e| e.message)?;
    let meta = fs::symlink_metadata(path).map_err(|e| io_message(&e))?;
    if !meta.file_type().is_dir() {
        return deleter.remove_file(path).map_err(|e| e.message());
    }
    // Tree: files first, then directories bottom-up; links are unlinked, never followed.
    let mut bytes = 0u64;
    let mut failures: Vec<String> = Vec::new();
    for entry in WalkDir::new(path)
        .follow_links(false)
        .same_file_system(true)
        .contents_first(true)
    {
        if job.is_cancelled() {
            failures.push("cancelled".into());
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                failures.push(e.to_string());
                continue;
            }
        };
        let p = entry.path();
        let res = if entry.file_type().is_dir() {
            deleter.remove_empty_dir(p).map(|_| 0)
        } else {
            deleter.remove_file(p)
        };
        match res {
            Ok(b) => bytes += b,
            Err(e) => failures.push(format!("{}: {}", p.display(), e.message())),
        }
    }
    if failures.is_empty() {
        Ok(bytes)
    } else {
        Err(format!(
            "{} item(s) could not be removed; first: {}",
            failures.len(),
            failures[0]
        ))
    }
}

/// Delete exactly the paths in `wanted` that appear in `allowed` (the server-side scan).
/// Everything else is reported as refused.
pub fn remove_verified(
    ctx: &Ctx,
    wanted: &[String],
    allowed: &[Leftover],
    job: &Job,
) -> Result<Vec<PathResult>> {
    let s = settings::load(ctx);
    let excludes = ExcludeSet::from_settings(&ctx.env, &s)?;
    let safety = Arc::new(Safety::new(&ctx.env, excludes));
    let allowed: HashSet<&str> = allowed.iter().map(|l| l.path.as_str()).collect();
    let mut results = Vec::new();
    let total = wanted.len();
    for (i, raw) in wanted.iter().enumerate() {
        job.check_cancelled()?;
        job.progress(
            crate::job::ProgressEvent::new("leftovers")
                .counts(i as u64, total as u64)
                .fraction(i as f64 / total.max(1) as f64)
                .message(raw.clone()),
        );
        if !allowed.contains(raw.as_str()) {
            results.push(PathResult {
                path: raw.clone(),
                ok: false,
                error: Some("refused: not a leftover of this application".into()),
                bytes: 0,
            });
            continue;
        }
        match delete_one(&safety, Path::new(raw), job) {
            Ok(bytes) => results.push(PathResult {
                path: raw.clone(),
                ok: true,
                error: None,
                bytes,
            }),
            Err(e) => results.push(PathResult {
                path: raw.clone(),
                ok: false,
                error: Some(e),
                bytes: 0,
            }),
        }
    }
    Ok(results)
}

/// Remove one AppImage file (plus the `.desktop` launchers that start exactly that file).
pub fn remove_appimage(ctx: &Ctx, path: &Path, job: &Job) -> Result<(u64, Vec<String>)> {
    let s = settings::load(ctx);
    let excludes = ExcludeSet::from_settings(&ctx.env, &s)?;
    let safety = Arc::new(Safety::new(&ctx.env, excludes));
    let bytes = delete_one(&safety, path, job).map_err(crate::error::ApiError::io)?;
    let mut removed = Vec::new();
    for d in desktop_entries(ctx, &HashSet::new(), Some(path)) {
        if delete_one(&safety, &d, job).is_ok() {
            removed.push(d.to_string_lossy().into_owned());
        }
    }
    Ok((bytes, removed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::MockRunner;

    fn ctx(os: Os) -> (tempfile::TempDir, Ctx) {
        let d = tempfile::tempdir().unwrap();
        let mut c = Ctx::test(d.path(), MockRunner::new());
        c.env.os = os;
        fs::create_dir_all(&c.env.home).unwrap();
        fs::create_dir_all(c.env.root.join("tmp")).unwrap();
        (d, c)
    }

    fn mk(dir: &Path, rel: &str) -> PathBuf {
        let p = dir.join(rel);
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("f.txt"), "content").unwrap();
        p
    }

    #[test]
    fn candidate_names_are_conservative() {
        assert_eq!(
            candidate_names("Mozilla Firefox (x64 en-US)", "windows:hklm64:k"),
            [
                "mozilla firefox",
                "mozilla-firefox",
                "mozilla_firefox",
                "mozillafirefox"
            ]
        );
        assert_eq!(candidate_names("GIMP", "dpkg:gimp"), ["gimp"]);
        // too short / generic / path-like names give nothing
        assert!(candidate_names("Go", "dpkg:go").is_empty());
        assert!(candidate_names("Config", "dpkg:config").is_empty());
        assert!(candidate_names("Cache", "x").is_empty());
        assert!(candidate_names("../etc", "x").is_empty());
        assert!(candidate_names("", "").is_empty());
        assert!(candidate_names("Microsoft", "x").is_empty());
        // package key adds its own name
        assert_eq!(
            candidate_names("Some App", "snap:some-app"),
            ["some app", "some-app", "some_app", "someapp"]
        );
    }

    #[test]
    fn bundle_id_validation() {
        assert!(valid_bundle_id("org.mozilla.firefox"));
        assert!(!valid_bundle_id("com.apple.finder"));
        assert!(!valid_bundle_id("COM.Apple.Safari"));
        assert!(!valid_bundle_id("../../etc"));
        assert!(!valid_bundle_id("a/b.c"));
        assert!(!valid_bundle_id("nodots"));
    }

    #[test]
    fn linux_finds_exact_named_folders_only() {
        let (_d, c) = ctx(Os::Linux);
        mk(&c.env.config_dir, "Obsidian");
        mk(&c.env.config_dir, "obsidian-other");
        mk(&c.env.cache_dir, "obsidian");
        mk(&c.env.user_data_dir, "Obsidian");
        mk(&c.env.home, ".obsidian");
        mk(&c.env.config_dir, "unrelated");
        let v = scan(&c, "Obsidian", "appimage:/x/Obsidian-1.AppImage", None);
        let mut names: Vec<String> = v
            .iter()
            .map(|l| {
                Path::new(&l.path)
                    .strip_prefix(&c.env.home)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                ".cache/obsidian",
                ".config/Obsidian",
                ".local/share/Obsidian",
                ".obsidian"
            ]
        );
        assert!(v.iter().all(|l| l.size_bytes >= 7));
        let kinds: Vec<&str> = v.iter().map(|l| l.kind.as_str()).collect();
        assert!(kinds.contains(&"cache") && kinds.contains(&"config") && kinds.contains(&"data"));
    }

    #[test]
    fn linux_desktop_launchers_and_flatpak_snap_dirs() {
        let (_d, c) = ctx(Os::Linux);
        let apps = c.env.user_data_dir.join("applications");
        fs::create_dir_all(&apps).unwrap();
        fs::write(
            apps.join("obsidian.desktop"),
            "[Desktop Entry]\nName=Obsidian\nExec=obsidian %U\n",
        )
        .unwrap();
        fs::write(
            apps.join("appimagekit_abc-Obsidian.desktop"),
            "[Desktop Entry]\nExec=\"/home/u/Applications/Obsidian-1.AppImage\" --no-sandbox %U\n",
        )
        .unwrap();
        fs::write(
            apps.join("other.desktop"),
            "[Desktop Entry]\nExec=/usr/bin/other\n",
        )
        .unwrap();
        fs::write(
            apps.join("prefix-obsidian.desktop"),
            "[Desktop Entry]\nExec=/usr/bin/obsidian-tool\n",
        )
        .unwrap();
        let v = scan(&c, "Obsidian", "dpkg:obsidian", None);
        let files: Vec<String> = v
            .iter()
            .map(|l| {
                Path::new(&l.path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(files, ["obsidian.desktop"]);
        assert!(v.iter().all(|l| l.kind == "launcher"));
        // exact AppImage path match
        let set = HashSet::new();
        let hits = desktop_entries(
            &c,
            &set,
            Some(Path::new("/home/u/Applications/Obsidian-1.AppImage")),
        );
        assert_eq!(hits.len(), 1);

        let fp = c.env.home.join(".var/app/org.gimp.GIMP");
        fs::create_dir_all(fp.join("config")).unwrap();
        let v = scan(&c, "GIMP", "flatpak:org.gimp.GIMP", None);
        assert!(v.iter().any(|l| Path::new(&l.path) == fp));
        let sn = c.env.home.join("snap/firefox");
        fs::create_dir_all(&sn).unwrap();
        let v = scan(&c, "Firefox", "snap:firefox", None);
        assert!(v.iter().any(|l| Path::new(&l.path) == sn));
    }

    #[test]
    fn windows_looks_in_appdata_and_programdata_only() {
        let (_d, c) = ctx(Os::Windows);
        mk(&c.env.user_data_dir, "Widget");
        mk(&c.env.data_local_dir, "widget");
        mk(&c.env.sys_path("/ProgramData"), "WIDGET");
        mk(&c.env.sys_path("/Program Files"), "Widget");
        let v = scan(&c, "Widget (x64)", "windows:hklm64:{X}", None);
        assert_eq!(v.len(), 3, "{v:?}");
        assert!(v.iter().all(|l| !l.path.contains("Program Files")));
        assert!(v.iter().any(|l| l.path.contains("ProgramData")));
    }

    #[test]
    fn macos_uses_library_folders_and_bundle_id() {
        let (_d, c) = ctx(Os::MacOs);
        let lib = c.env.home.join("Library");
        mk(&lib, "Application Support/Widget");
        mk(&lib, "Caches/org.example.widget");
        mk(&lib, "Logs/Widget");
        fs::create_dir_all(lib.join("Preferences")).unwrap();
        fs::write(lib.join("Preferences/org.example.widget.plist"), "x").unwrap();
        fs::write(lib.join("Preferences/org.example.other.plist"), "x").unwrap();
        mk(
            &lib,
            "Saved Application State/org.example.widget.savedState",
        );
        let v = scan(
            &c,
            "Widget",
            "macapp:/Applications/Widget.app",
            Some("org.example.widget"),
        );
        let rel: Vec<String> = v
            .iter()
            .map(|l| {
                Path::new(&l.path)
                    .strip_prefix(&lib)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            rel,
            [
                "Application Support/Widget",
                "Caches/org.example.widget",
                "Logs/Widget",
                "Preferences/org.example.widget.plist",
                "Saved Application State/org.example.widget.savedState",
            ]
        );
        // an Apple bundle id from the client is ignored
        let v = scan(&c, "Widget", "macapp:/x", Some("com.apple.Safari"));
        assert!(v.iter().all(|l| !l.path.contains("Safari")));
    }

    #[test]
    fn protected_and_excluded_paths_are_never_candidates() {
        let (_d, c) = ctx(Os::Linux);
        // A folder named like the app directly in $HOME's protected set: Documents/Downloads
        // are generic names, and a symlink is skipped.
        let target = mk(&c.env.home, "real-target");
        fs::create_dir_all(&c.env.config_dir).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, c.env.config_dir.join("linked-app")).unwrap();
        mk(&c.env.config_dir, "excluded-app");
        let excl = c.env.config_dir.join("excluded-app");
        settings::update(&c, |s| {
            s.exclude.push(settings::ExcludeEntry {
                id: "e1".into(),
                pattern: excl.to_string_lossy().into_owned(),
            })
        })
        .unwrap();
        assert!(scan(&c, "linked-app", "x", None).is_empty());
        assert!(scan(&c, "excluded-app", "x", None).is_empty());
        assert!(scan(&c, "Documents", "x", None).is_empty());
    }

    #[test]
    fn remove_verified_only_deletes_scanned_paths() {
        let (_d, c) = ctx(Os::Linux);
        let mine = mk(&c.env.config_dir, "Obsidian");
        fs::create_dir_all(mine.join("sub/deeper")).unwrap();
        fs::write(mine.join("sub/deeper/x"), "data").unwrap();
        let unrelated = mk(&c.env.config_dir, "unrelated");
        let docs = c.env.home.join("Documents");
        fs::create_dir_all(&docs).unwrap();
        fs::write(docs.join("precious.txt"), "keep").unwrap();
        let allowed = scan(&c, "Obsidian", "dpkg:obsidian", None);
        assert_eq!(allowed.len(), 1);
        let wanted = vec![
            mine.to_string_lossy().into_owned(),
            unrelated.to_string_lossy().into_owned(),
            docs.to_string_lossy().into_owned(),
            "/etc".to_string(),
            "relative/path".to_string(),
        ];
        let res = remove_verified(&c, &wanted, &allowed, &Job::detached()).unwrap();
        assert!(res[0].ok, "{:?}", res[0]);
        assert!(res[0].bytes >= 11);
        for r in &res[1..] {
            assert!(!r.ok);
            assert!(r.error.as_deref().unwrap().starts_with("refused"), "{r:?}");
        }
        assert!(!mine.exists());
        assert!(unrelated.exists());
        assert!(docs.join("precious.txt").exists());
    }

    #[test]
    fn remove_verified_still_applies_safe_deleter_to_allowed_paths() {
        // Even if a (buggy) scan returned a protected path, SafeDeleter refuses it.
        let (_d, c) = ctx(Os::Linux);
        let docs = c.env.home.join("Documents");
        fs::create_dir_all(&docs).unwrap();
        fs::write(docs.join("a.txt"), "x").unwrap();
        let fake = vec![Leftover {
            path: docs.to_string_lossy().into_owned(),
            size_bytes: 1,
            kind: "data".into(),
        }];
        let res = remove_verified(&c, &[fake[0].path.clone()], &fake, &Job::detached()).unwrap();
        assert!(!res[0].ok);
        assert!(docs.join("a.txt").exists());
    }

    #[test]
    fn remove_appimage_removes_file_and_matching_launcher() {
        let (_d, c) = ctx(Os::Linux);
        let apps = c.env.home.join("Applications");
        fs::create_dir_all(&apps).unwrap();
        let img = apps.join("Foo-1.0.AppImage");
        fs::write(&img, "img").unwrap();
        let dd = c.env.user_data_dir.join("applications");
        fs::create_dir_all(&dd).unwrap();
        let other = dd.join("other.desktop");
        let mine = dd.join("appimagekit_foo.desktop");
        fs::write(
            &mine,
            format!("[Desktop Entry]\nExec={} %U\n", img.display()),
        )
        .unwrap();
        fs::write(&other, "[Desktop Entry]\nExec=/usr/bin/other\n").unwrap();
        let (bytes, removed) = remove_appimage(&c, &img, &Job::detached()).unwrap();
        assert_eq!(bytes, 3);
        assert_eq!(removed, vec![mine.to_string_lossy().into_owned()]);
        assert!(!img.exists() && !mine.exists() && other.exists());
    }
}
