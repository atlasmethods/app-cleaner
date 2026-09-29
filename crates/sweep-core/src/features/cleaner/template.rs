//! Path templates: `{var}/rest/of/path`, with `*`-style globs allowed in components.
//!
//! Templates come only from the embedded rule files and are expanded only from
//! [`Env`]; nothing a client sends is ever used as a template.

use globset::GlobBuilder;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ctx::Env;
use crate::features::cleaner::firefox;

/// Every variable a template may start with.
pub const VARS: &[&str] = &[
    "home",
    "config",
    "cache",
    "data",
    "temp",
    "sys",
    "appdata",
    "localappdata",
    "library",
    "firefox_profile",
];

/// Variables that only make sense on one OS: (variable, os name as used in rule files).
pub const OS_ONLY_VARS: &[(&str, &str)] = &[
    ("appdata", "windows"),
    ("localappdata", "windows"),
    ("library", "macos"),
];

const CASE_INSENSITIVE: bool = cfg!(any(windows, target_os = "macos"));

/// Split `{var}/a/b` into (`var`, [`a`, `b`]). `Err` describes what is wrong.
pub fn parse(t: &str) -> Result<(&str, Vec<&str>), String> {
    let rest = t
        .strip_prefix('{')
        .ok_or_else(|| format!("template must start with a {{variable}}: `{t}`"))?;
    let (var, after) = rest
        .split_once('}')
        .ok_or_else(|| format!("unterminated variable in `{t}`"))?;
    if !VARS.contains(&var) {
        return Err(format!("unknown template variable `{{{var}}}` in `{t}`"));
    }
    if t.contains('\\') {
        return Err(format!("use `/` as separator in templates: `{t}`"));
    }
    let comps: Vec<&str> = match after {
        "" => Vec::new(),
        a => {
            let a = a
                .strip_prefix('/')
                .ok_or_else(|| format!("expected `/` after variable in `{t}`"))?;
            a.split('/').collect()
        }
    };
    for c in &comps {
        if c.is_empty() || *c == "." || *c == ".." {
            return Err(format!("bad path component `{c}` in `{t}`"));
        }
        if c.contains('{') && !c.contains(',') {
            return Err(format!("stray `{{` in component `{c}` of `{t}`"));
        }
    }
    Ok((var, comps))
}

fn has_glob(c: &str) -> bool {
    c.contains(['*', '?', '[', '{'])
}

fn prefixes(env: &Env, var: &str) -> Vec<PathBuf> {
    match var {
        "home" => vec![env.home.clone()],
        "config" => vec![env.config_dir.clone()],
        "cache" => vec![env.cache_dir.clone()],
        "data" => vec![env.user_data_dir.clone()],
        "temp" => vec![env.temp_dir.clone()],
        "sys" => vec![env.root.clone()],
        "appdata" => vec![env.config_dir.clone()],
        "localappdata" => vec![env.data_local_dir.clone()],
        "library" => vec![env.home.join("Library")],
        "firefox_profile" => firefox::profile_dirs(env),
        _ => Vec::new(),
    }
}

/// An existing filesystem entry a template resolved to.
#[derive(Debug)]
pub struct Found {
    pub path: PathBuf,
    /// `symlink_metadata` of `path` (links are never followed).
    pub meta: fs::Metadata,
}

/// Expand a template into existing entries, sorted by path. Directories reached through
/// a glob component must be real directories (symlinked profile dirs are ignored).
pub fn resolve(env: &Env, template: &str) -> Vec<Found> {
    let Ok((var, comps)) = parse(template) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for prefix in prefixes(env, var) {
        let mut current = vec![prefix];
        for (i, comp) in comps.iter().enumerate() {
            let last = i + 1 == comps.len();
            let mut next = Vec::new();
            if has_glob(comp) {
                let Ok(g) = GlobBuilder::new(comp)
                    .case_insensitive(CASE_INSENSITIVE)
                    .literal_separator(true)
                    .build()
                else {
                    return Vec::new();
                };
                let m = g.compile_matcher();
                for cur in &current {
                    let Ok(rd) = fs::read_dir(cur) else { continue };
                    for e in rd.flatten() {
                        let name = e.file_name();
                        if !m.is_match(Path::new(&name)) {
                            continue;
                        }
                        let p = cur.join(&name);
                        if !last {
                            match fs::symlink_metadata(&p) {
                                Ok(md) if md.file_type().is_dir() => {}
                                _ => continue,
                            }
                        }
                        next.push(p);
                    }
                }
            } else {
                next.extend(current.iter().map(|c| c.join(comp)));
            }
            current = next;
        }
        for p in current {
            if let Ok(meta) = fs::symlink_metadata(&p) {
                out.push(Found { path: p, meta });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn env() -> (tempfile::TempDir, Env) {
        let d = tempfile::tempdir().unwrap();
        let e = Env::for_test(d.path());
        fs::create_dir_all(&e.home).unwrap();
        (d, e)
    }

    #[test]
    fn parse_accepts_and_rejects() {
        assert!(parse("{home}/a/b").is_ok());
        assert!(parse("{sys}").is_ok());
        assert!(parse("{config}/google-chrome/*/Cache").is_ok());
        assert!(parse("/abs/path").is_err());
        assert!(parse("{nope}/a").is_err());
        assert!(parse("{home}a").is_err());
        assert!(parse("{home}/a/../b").is_err());
        assert!(parse("{home}//b").is_err());
        assert!(parse("{home}/a\\b").is_err());
        assert!(parse("{home").is_err());
    }

    #[test]
    fn resolves_literals_and_globs_and_only_existing() {
        let (_d, e) = env();
        for p in [
            "Default/Cache",
            "Profile 1/Cache",
            "Profile 2/Other",
            "Crashpad",
        ] {
            fs::create_dir_all(e.config_dir.join("google-chrome").join(p)).unwrap();
        }
        let r = resolve(&e, "{config}/google-chrome/*/Cache");
        let names: Vec<_> = r
            .iter()
            .map(|f| f.path.strip_prefix(&e.config_dir).unwrap().to_owned())
            .collect();
        assert_eq!(
            names,
            vec![
                PathBuf::from("google-chrome/Default/Cache"),
                PathBuf::from("google-chrome/Profile 1/Cache")
            ]
        );
        assert!(resolve(&e, "{config}/nothing/here").is_empty());
        assert_eq!(resolve(&e, "{home}").len(), 1);
        // sys is the redirected root
        fs::create_dir_all(e.sys_path("/var/cache/apt/archives")).unwrap();
        assert_eq!(resolve(&e, "{sys}/var/cache/apt/archives").len(), 1);
    }

    #[test]
    fn glob_matched_intermediate_symlinks_are_ignored() {
        let (_d, e) = env();
        let real = e.home.join("real");
        fs::create_dir_all(real.join("Cache")).unwrap();
        let root = e.config_dir.join("app");
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&real, root.join("linked-profile")).unwrap();
        fs::create_dir_all(root.join("Default/Cache")).unwrap();
        let r = resolve(&e, "{config}/app/*/Cache");
        assert_eq!(r.len(), 1);
        assert!(r[0].path.ends_with("Default/Cache"));
    }

    #[test]
    fn final_component_symlink_is_reported_as_symlink() {
        let (_d, e) = env();
        let target = e.home.join("target");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir_all(e.cache_dir.join("app")).unwrap();
        std::os::unix::fs::symlink(&target, e.cache_dir.join("app/Cache")).unwrap();
        let r = resolve(&e, "{cache}/app/Cache");
        assert_eq!(r.len(), 1);
        assert!(r[0].meta.file_type().is_symlink());
    }
}
