//! Firefox profile discovery: `profiles.ini` first, then a glob-style fallback.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ctx::{Env, Os};
use crate::safety::{key_of_path, normalize};

/// Directories that may contain `profiles.ini` for the current OS.
fn roots(env: &Env) -> Vec<PathBuf> {
    match env.os {
        Os::Linux => vec![
            env.home.join(".mozilla").join("firefox"),
            env.home.join("snap/firefox/common/.mozilla/firefox"),
            env.home
                .join(".var/app/org.mozilla.firefox/.mozilla/firefox"),
        ],
        Os::Windows => vec![env.config_dir.join("Mozilla").join("Firefox")],
        Os::MacOs => vec![env.config_dir.join("Firefox")],
    }
}

fn is_real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false)
}

/// Parse `profiles.ini` into profile directories (relative ones resolved against `root`).
pub fn parse_profiles_ini(root: &Path, text: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut in_profile = false;
    let mut path: Option<String> = None;
    let mut relative = true;
    let flush = |path: &mut Option<String>, relative: &mut bool, out: &mut Vec<PathBuf>| {
        if let Some(p) = path.take() {
            let pb = PathBuf::from(p.replace('\\', "/"));
            if *relative {
                let mut full = root.to_path_buf();
                for c in pb.components() {
                    if let std::path::Component::Normal(n) = c {
                        full.push(n);
                    }
                }
                out.push(full);
            } else if pb.is_absolute() || p.contains(':') {
                out.push(PathBuf::from(p));
            }
        }
        *relative = true;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            flush(&mut path, &mut relative, &mut out);
            in_profile = line[1..line.len() - 1].starts_with("Profile");
            continue;
        }
        if !in_profile {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "Path" => path = Some(v.trim().to_string()),
                "IsRelative" => relative = v.trim() != "0",
                _ => {}
            }
        }
    }
    flush(&mut path, &mut relative, &mut out);
    out
}

fn looks_like_profile(dir: &Path) -> bool {
    ["prefs.js", "places.sqlite", "cookies.sqlite", "times.json"]
        .iter()
        .any(|f| dir.join(f).exists())
}

/// All existing Firefox profile directories: from `profiles.ini` plus any directory in
/// the profile roots that looks like a profile (name contains a dot and holds profile
/// files). Symlinked profile directories are ignored. Sorted, de-duplicated.
pub fn profile_dirs(env: &Env) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for root in roots(env) {
        if let Ok(text) = fs::read_to_string(root.join("profiles.ini")) {
            found.extend(parse_profiles_ini(&root, &text));
        }
        let scan_dirs = match env.os {
            Os::Linux => vec![root.clone()],
            _ => vec![root.join("Profiles"), root.clone()],
        };
        for dir in scan_dirs {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if name.contains('.') && is_real_dir(&p) && looks_like_profile(&p) {
                    found.push(p);
                }
            }
        }
    }
    let mut seen = HashSet::new();
    let mut out: Vec<PathBuf> = found
        .into_iter()
        .map(|p| normalize(&p))
        .filter(|p| is_real_dir(p))
        .filter(|p| seen.insert(key_of_path(p)))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relative_absolute_and_ignores_install_sections() {
        let root = Path::new("/home/u/.mozilla/firefox");
        let ini = "[Install4F96D1932A9F858E]\nDefault=Profiles/aaa.default-release\nLocked=1\n\n\
                   [Profile1]\nName=work\nIsRelative=0\nPath=/data/ff-work\n\n\
                   [Profile0]\nName=default\nIsRelative=1\nPath=Profiles/aaa.default-release\nDefault=1\n\n\
                   [General]\nStartWithLastProfile=1\n";
        let p = parse_profiles_ini(root, ini);
        assert_eq!(
            p,
            vec![
                PathBuf::from("/data/ff-work"),
                PathBuf::from("/home/u/.mozilla/firefox/Profiles/aaa.default-release"),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn discovers_via_ini_and_glob_fallback_dedupes_and_skips_junk() {
        let d = tempfile::tempdir().unwrap();
        let mut e = Env::for_test(d.path());
        e.os = Os::Linux;
        let root = e.home.join(".mozilla/firefox");
        fs::create_dir_all(root.join("aaa.default-release")).unwrap();
        fs::write(root.join("aaa.default-release/places.sqlite"), "").unwrap();
        // only found by the fallback (not in profiles.ini)
        fs::create_dir_all(root.join("bbb.dev-edition")).unwrap();
        fs::write(root.join("bbb.dev-edition/prefs.js"), "").unwrap();
        // junk that must not be treated as a profile
        fs::create_dir_all(root.join("Crash Reports")).unwrap();
        fs::create_dir_all(root.join("ccc.empty")).unwrap();
        // custom location from the ini
        let custom = d.path().join("custom-profile");
        fs::create_dir_all(&custom).unwrap();
        fs::write(
            root.join("profiles.ini"),
            format!(
                "[Profile0]\nIsRelative=1\nPath=aaa.default-release\n[Profile1]\nIsRelative=0\nPath={}\n",
                custom.display()
            ),
        )
        .unwrap();
        // a nonexistent one in the ini is dropped
        let mut ini = fs::read_to_string(root.join("profiles.ini")).unwrap();
        ini.push_str("[Profile2]\nIsRelative=1\nPath=gone.profile\n");
        fs::write(root.join("profiles.ini"), ini).unwrap();

        let got = profile_dirs(&e);
        let mut want = vec![
            root.join("aaa.default-release"),
            root.join("bbb.dev-edition"),
            custom,
        ];
        want.sort();
        assert_eq!(got, want);
    }
}
