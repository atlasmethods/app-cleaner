//! Safety layer used by every destructive operation in ClearSweep.
//!
//! * [`Protected`] / [`is_protected`]: paths that must never be deleted (filesystem
//!   roots, the home directory, OS directories, `~/.ssh`, ...).
//! * [`ExcludeSet`]: the user's exclusion list (absolute paths or globs).
//! * [`SafeDeleter`]: the only way features remove files. It resolves the *parent* of
//!   the target (never the leaf, so a symlink is removed as a link and never followed),
//!   demands that the result lies strictly below an allowed base directory, and refuses
//!   protected and excluded paths.
//!
//! Directory walking anywhere in the app must use `walkdir` with
//! `follow_links(false)` and `same_file_system(true)`.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::ctx::{Env, Os};
use crate::error::{ApiError, Result};
use crate::features::settings::Settings;

/// Whether path comparison must ignore case (Windows and default macOS volumes).
const CASE_INSENSITIVE: bool = cfg!(any(windows, target_os = "macos"));

/// Lexically normalize: drop `.`, resolve `..` where possible. Never touches the disk.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::ParentDir) | None => out.push(".."),
                // `..` at the root stays at the root.
                Some(_) => {}
            },
            Component::Normal(p) => out.push(p),
        }
    }
    out
}

/// Strip the Windows verbatim prefix (`\\?\C:\x` -> `C:\x`) so canonical paths compare
/// equal to the paths we derive from the environment.
fn simplify(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            let b = rest.as_bytes();
            if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                return PathBuf::from(rest.to_string());
            }
        }
    }
    p
}

/// `fs::canonicalize` with verbatim prefixes removed.
pub fn canonicalize(p: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(p).map(simplify)
}

/// Comparable form of a normalized path: components, lower-cased when the FS is
/// case-insensitive.
fn key_of(p: &Path) -> Vec<String> {
    p.components()
        .map(|c| {
            let s = c.as_os_str().to_string_lossy();
            if CASE_INSENSITIVE {
                s.to_lowercase()
            } else {
                s.into_owned()
            }
        })
        .collect()
}

/// Comparable key for an arbitrary path (normalized, case-folded where needed).
pub fn key_of_path(p: &Path) -> Vec<String> {
    key_of(&normalize(p))
}

fn key_starts_with(key: &[String], prefix: &[String]) -> bool {
    key.len() >= prefix.len() && key[..prefix.len()] == *prefix
}

/// True when `child` is `base` or lies below it (component-wise, after normalization).
pub fn is_within(child: &Path, base: &Path) -> bool {
    key_starts_with(&key_of(&normalize(child)), &key_of(&normalize(base)))
}

/// Does the normalized path have no directory components at all (`/`, `C:\`, `C:`)?
fn is_fs_root(p: &Path) -> bool {
    !p.components().any(|c| matches!(c, Component::Normal(_)))
}

/// Expand a leading `~` to the home directory.
pub fn expand_tilde(env: &Env, s: &str) -> PathBuf {
    if s == "~" {
        return env.home.clone();
    }
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        let mut p = env.home.clone();
        for part in rest.split(['/', '\\']).filter(|x| !x.is_empty()) {
            p.push(part);
        }
        return p;
    }
    PathBuf::from(s)
}

// ---------------------------------------------------------------- protected paths

/// The set of paths that must never be deleted.
pub struct Protected {
    exact: HashSet<Vec<String>>,
    trees: Vec<Vec<String>>,
}

impl Protected {
    pub fn new(env: &Env) -> Self {
        let mut exact: Vec<PathBuf> = vec![env.root.clone(), env.home.clone()];
        let mut trees: Vec<PathBuf> = Vec::new();

        for d in [
            "Desktop",
            "Documents",
            "Downloads",
            "Pictures",
            "Music",
            "Videos",
            "Movies",
            "Public",
            "Library",
        ] {
            exact.push(env.home.join(d));
        }
        for d in [".ssh", ".gnupg"] {
            trees.push(env.home.join(d));
        }
        // ClearSweep's own data (settings, history, restore points).
        trees.push(env.data_dir.clone());

        let os_dirs: &[&str] = match env.os {
            Os::Windows => &[
                "/Windows",
                "/Program Files",
                "/Program Files (x86)",
                "/ProgramData",
                "/Users",
            ],
            Os::Linux => &[
                "/bin", "/boot", "/dev", "/etc", "/lib", "/lib64", "/lib32", "/proc", "/sbin",
                "/sys", "/usr", "/var", "/opt", "/home", "/root",
            ],
            Os::MacOs => &[
                "/bin",
                "/dev",
                "/etc",
                "/sbin",
                "/usr",
                "/var",
                "/opt",
                "/private",
                "/Applications",
                "/System",
                "/Library",
                "/Users",
                "/Volumes",
            ],
        };
        for d in os_dirs {
            exact.push(env.sys_path(d));
        }

        let mut set = HashSet::new();
        let mut add_exact = |p: &Path| {
            set.insert(key_of(&normalize(p)));
        };
        for p in &exact {
            add_exact(p);
            if let Ok(c) = canonicalize(p) {
                add_exact(&c);
            }
        }
        let mut tree_keys = Vec::new();
        for p in &trees {
            tree_keys.push(key_of(&normalize(p)));
            if let Ok(c) = canonicalize(p) {
                tree_keys.push(key_of(&c));
            }
        }
        Protected {
            exact: set,
            trees: tree_keys,
        }
    }

    /// Lexical check (the caller resolves symlinks in the parent first when it matters).
    pub fn is_protected(&self, path: &Path) -> bool {
        let n = normalize(path);
        if n.as_os_str().is_empty() || is_fs_root(&n) {
            return true;
        }
        let k = key_of(&n);
        self.exact.contains(&k) || self.trees.iter().any(|t| key_starts_with(&k, t))
    }
}

/// Convenience one-shot check. Prefer building a [`Protected`] once in loops.
pub fn is_protected(env: &Env, path: &Path) -> bool {
    Protected::new(env).is_protected(path)
}

// ---------------------------------------------------------------- exclusions

/// The user's exclusion list. A pattern is an absolute path or glob (`~` expanded); a
/// pattern naming a directory excludes everything beneath it.
#[derive(Default)]
pub struct ExcludeSet {
    globs: Option<GlobSet>,
    literals: HashSet<Vec<String>>,
    count: usize,
}

fn slashify(s: &str) -> String {
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.to_string()
    }
}

fn path_str_for_glob(p: &Path) -> String {
    slashify(&p.to_string_lossy())
}

/// Validate an exclusion pattern; returns the expanded absolute form.
pub fn validate_exclude_pattern(env: &Env, pattern: &str) -> Result<PathBuf> {
    let pat = pattern.trim();
    if pat.is_empty() {
        return Err(ApiError::invalid_params("exclusion pattern is empty"));
    }
    let expanded = expand_tilde(env, pat);
    if !expanded.is_absolute() {
        return Err(ApiError::invalid_params(format!(
            "exclusion pattern must be an absolute path or start with ~: {pat}"
        )));
    }
    if expanded
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(ApiError::invalid_params(format!(
            "exclusion pattern must not contain `..`: {pat}"
        )));
    }
    GlobBuilder::new(&path_str_for_glob(&expanded))
        .literal_separator(true)
        .build()
        .map_err(|e| ApiError::invalid_params(format!("invalid exclusion pattern `{pat}`: {e}")))?;
    Ok(expanded)
}

impl ExcludeSet {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Fails (closed) if any pattern is invalid: silently dropping an exclusion could
    /// delete something the user asked us to keep.
    pub fn from_patterns<S: AsRef<str>>(env: &Env, patterns: &[S]) -> Result<Self> {
        let mut b = GlobSetBuilder::new();
        let mut literals = HashSet::new();
        for p in patterns {
            let expanded = validate_exclude_pattern(env, p.as_ref())?;
            let g = GlobBuilder::new(&path_str_for_glob(&expanded))
                .literal_separator(true)
                .case_insensitive(CASE_INSENSITIVE)
                .build()
                .map_err(|e| ApiError::invalid_params(e.to_string()))?;
            b.add(g);
            literals.insert(key_of(&normalize(&expanded)));
        }
        let globs = b
            .build()
            .map_err(|e| ApiError::invalid_params(e.to_string()))?;
        Ok(ExcludeSet {
            count: patterns.len(),
            globs: Some(globs),
            literals,
        })
    }

    pub fn from_settings(env: &Env, s: &Settings) -> Result<Self> {
        let pats: Vec<&str> = s.exclude.iter().map(|e| e.pattern.as_str()).collect();
        Self::from_patterns(env, &pats)
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// True if `path` or any of its ancestors matches an exclusion.
    pub fn is_excluded(&self, path: &Path) -> bool {
        if self.count == 0 {
            return false;
        }
        let n = normalize(path);
        for anc in n.ancestors() {
            if anc.as_os_str().is_empty() {
                break;
            }
            if self.literals.contains(&key_of(anc)) {
                return true;
            }
            if let Some(g) = &self.globs {
                if g.is_match(path_str_for_glob(anc)) {
                    return true;
                }
            }
        }
        false
    }
}

// ---------------------------------------------------------------- SafeDeleter

/// Why a deletion was refused or failed.
#[derive(Debug)]
pub enum SafeError {
    /// Refused by policy (reason text is user-presentable).
    Refused(Refusal, PathBuf),
    Io(io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotAbsolute,
    NoFileName,
    OutsideBase,
    Protected,
    Excluded,
    IsDirectory,
    NotADirectory,
}

impl Refusal {
    pub fn text(self) -> &'static str {
        match self {
            Refusal::NotAbsolute => "path is not absolute",
            Refusal::NoFileName => "path has no file name",
            Refusal::OutsideBase => "path is outside the allowed folder",
            Refusal::Protected => "path is protected",
            Refusal::Excluded => "path is on the exclusion list",
            Refusal::IsDirectory => "path is a directory",
            Refusal::NotADirectory => "path is not a directory",
        }
    }
}

impl SafeError {
    pub fn is_excluded(&self) -> bool {
        matches!(self, SafeError::Refused(Refusal::Excluded, _))
    }
    /// The target vanished before we got to it (benign race with the owning app).
    pub fn is_not_found(&self) -> bool {
        matches!(self, SafeError::Io(e) if e.kind() == io::ErrorKind::NotFound)
    }
    pub fn message(&self) -> String {
        match self {
            SafeError::Refused(r, _) => r.text().to_string(),
            SafeError::Io(e) => io_message(e),
        }
    }
    pub fn to_api(&self, path: &Path) -> ApiError {
        match self {
            SafeError::Refused(r, _) => {
                ApiError::permission_denied(format!("{}: {}", r.text(), path.display()))
            }
            SafeError::Io(e) => ApiError::from(io::Error::new(e.kind(), e.to_string())),
        }
    }
}

impl From<io::Error> for SafeError {
    fn from(e: io::Error) -> Self {
        SafeError::Io(e)
    }
}

/// Short human-readable text for an I/O error (no OS error number noise for the common cases).
pub fn io_message(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        io::ErrorKind::NotFound => "no longer exists".to_string(),
        _ => e.to_string(),
    }
}

/// Shared, immutable safety configuration for one operation.
pub struct Safety {
    pub protected: Protected,
    pub excludes: ExcludeSet,
}

impl Safety {
    pub fn new(env: &Env, excludes: ExcludeSet) -> Self {
        Safety {
            protected: Protected::new(env),
            excludes,
        }
    }
}

/// Result of resolving a candidate: the canonical-parent-based path and the leaf's own
/// (non-followed) metadata.
pub struct Resolved {
    pub path: PathBuf,
    pub meta: fs::Metadata,
}

pub struct SafeDeleter {
    safety: Arc<Safety>,
    base: PathBuf,
    base_key: Vec<String>,
    /// `Some(passes)` = overwrite regular files before unlinking.
    secure_passes: Option<u32>,
}

impl SafeDeleter {
    /// `allowed_base` must exist; it is canonicalized. Deletion of the base itself, or
    /// of anything outside it, is refused. A protected base is refused outright.
    pub fn new(env: &Env, allowed_base: &Path, excludes: ExcludeSet) -> Result<Self> {
        Self::with_safety(Arc::new(Safety::new(env, excludes)), allowed_base)
    }

    pub fn with_safety(safety: Arc<Safety>, allowed_base: &Path) -> Result<Self> {
        Self::build(safety, allowed_base, false)
    }

    /// For explicit, user-selected targets (secure delete): the base is only the parent
    /// directory of the selection, which may legitimately be a protected folder such as
    /// `~/Documents`. Every individual path is still checked against the protected set.
    pub fn for_selection(safety: Arc<Safety>, parent_dir: &Path) -> Result<Self> {
        Self::build(safety, parent_dir, true)
    }

    fn build(safety: Arc<Safety>, allowed_base: &Path, allow_protected_base: bool) -> Result<Self> {
        if !allowed_base.is_absolute() {
            return Err(ApiError::invalid_params(format!(
                "base folder is not absolute: {}",
                allowed_base.display()
            )));
        }
        let base = canonicalize(allowed_base).map_err(|e| {
            ApiError::from(io::Error::new(
                e.kind(),
                format!("base folder {}: {}", allowed_base.display(), io_message(&e)),
            ))
        })?;
        if !allow_protected_base
            && (safety.protected.is_protected(&base)
                || safety.protected.is_protected(&normalize(allowed_base)))
        {
            return Err(ApiError::permission_denied(format!(
                "refusing to clean inside a protected folder: {}",
                allowed_base.display()
            )));
        }
        let base_key = key_of(&base);
        Ok(SafeDeleter {
            safety,
            base,
            base_key,
            secure_passes: None,
        })
    }

    /// Overwrite regular files with `passes` passes before unlinking them.
    pub fn with_secure(mut self, passes: u32) -> Self {
        self.secure_passes = Some(passes);
        self
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    pub fn safety(&self) -> &Arc<Safety> {
        &self.safety
    }

    /// Resolve `path` and apply every policy check without touching the filesystem
    /// (other than reading metadata). Used by analysis so it reports exactly what a
    /// real deletion would accept.
    pub fn check(&self, path: &Path) -> std::result::Result<Resolved, SafeError> {
        let refuse = |r: Refusal| SafeError::Refused(r, path.to_path_buf());
        if !path.is_absolute() {
            return Err(refuse(Refusal::NotAbsolute));
        }
        let lex = normalize(path);
        let name = lex
            .file_name()
            .ok_or_else(|| refuse(Refusal::NoFileName))?
            .to_os_string();
        let parent = lex.parent().ok_or_else(|| refuse(Refusal::NoFileName))?;
        // Canonicalize the PARENT only: the leaf may be a symlink and must stay one.
        let resolved = canonicalize(parent)?.join(&name);
        let rkey = key_of(&resolved);
        if rkey.len() <= self.base_key.len() || !key_starts_with(&rkey, &self.base_key) {
            return Err(refuse(Refusal::OutsideBase));
        }
        if self.safety.protected.is_protected(&resolved) || self.safety.protected.is_protected(&lex)
        {
            return Err(refuse(Refusal::Protected));
        }
        if self.safety.excludes.is_excluded(&resolved) || self.safety.excludes.is_excluded(&lex) {
            return Err(refuse(Refusal::Excluded));
        }
        let meta = fs::symlink_metadata(&resolved)?;
        Ok(Resolved {
            path: resolved,
            meta,
        })
    }

    /// Remove a file or symlink (never a real directory). Returns the bytes freed
    /// (0 for symlinks and special files).
    pub fn remove_file(&self, path: &Path) -> std::result::Result<u64, SafeError> {
        let r = self.check(path)?;
        let ft = r.meta.file_type();
        if ft.is_dir() {
            return Err(SafeError::Refused(Refusal::IsDirectory, path.to_path_buf()));
        }
        let bytes = if ft.is_file() { r.meta.len() } else { 0 };
        if ft.is_file() {
            if let Some(passes) = self.secure_passes {
                crate::features::secure_delete::secure_delete_file(
                    &r.path,
                    passes,
                    &crate::job::Job::detached(),
                )
                .map_err(|e| SafeError::Io(io::Error::other(e.message)))?;
                return Ok(bytes);
            }
        }
        remove_link_or_file(&r.path, &r.meta)?;
        Ok(bytes)
    }

    /// Remove an empty directory (`remove_dir` fails on non-empty ones).
    pub fn remove_empty_dir(&self, path: &Path) -> std::result::Result<(), SafeError> {
        let r = self.check(path)?;
        if !r.meta.file_type().is_dir() {
            return Err(SafeError::Refused(
                Refusal::NotADirectory,
                path.to_path_buf(),
            ));
        }
        fs::remove_dir(&r.path)?;
        Ok(())
    }
}

fn remove_link_or_file(path: &Path, meta: &fs::Metadata) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        if meta.file_type().is_symlink_dir() {
            return fs::remove_dir(path);
        }
    }
    let _ = meta;
    fs::remove_file(path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn env() -> (tempfile::TempDir, Env) {
        let d = tempfile::tempdir().unwrap();
        let e = Env::for_test(d.path());
        fs::create_dir_all(&e.home).unwrap();
        fs::create_dir_all(&e.root).unwrap();
        (d, e)
    }

    #[test]
    fn normalize_is_lexical() {
        assert_eq!(normalize(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normalize(Path::new("/../..")), PathBuf::from("/"));
        assert_eq!(normalize(Path::new("a/../../b")), PathBuf::from("../b"));
    }

    #[test]
    fn within_is_component_wise() {
        assert!(is_within(Path::new("/a/b/c"), Path::new("/a/b")));
        assert!(is_within(Path::new("/a/b"), Path::new("/a/b")));
        assert!(!is_within(Path::new("/a/bc"), Path::new("/a/b")));
        assert!(!is_within(Path::new("/a/b/../x"), Path::new("/a/b")));
    }

    #[test]
    fn protected_paths() {
        let (_d, e) = env();
        let p = Protected::new(&e);
        for path in [
            PathBuf::from("/"),
            e.root.clone(),
            e.home.clone(),
            e.home.join("Documents"),
            e.home.join("Downloads"),
            e.home.join("Desktop"),
            e.home.join(".ssh"),
            e.home.join(".ssh").join("id_ed25519"),
            e.home.join(".gnupg").join("private-keys-v1.d").join("k"),
            e.sys_path("/etc"),
            e.sys_path("/usr"),
            e.sys_path("/var"),
            e.sys_path("/home"),
            e.sys_path("/boot"),
            e.data_dir.clone(),
            e.data_dir.join("settings.json"),
            // lexical tricks
            e.home.join("x").join(".."),
            e.home.join("Documents").join("."),
        ] {
            assert!(p.is_protected(&path), "{path:?} should be protected");
        }
        for path in [
            e.home.join("Documents").join("a.txt"),
            e.home.join(".cache").join("x"),
            e.sys_path("/var/cache/apt/archives/a.deb"),
            e.sys_path("/tmp"),
            e.sys_path("/usr/share/x"),
        ] {
            assert!(!p.is_protected(&path), "{path:?} should be allowed");
        }
        assert!(is_protected(&e, &e.home));
    }

    #[test]
    fn protected_windows_dirs_when_os_is_windows() {
        let (_d, mut e) = env();
        e.os = Os::Windows;
        let p = Protected::new(&e);
        for d in [
            "/Windows",
            "/Program Files",
            "/Program Files (x86)",
            "/ProgramData",
            "/Users",
        ] {
            assert!(p.is_protected(&e.sys_path(d)), "{d}");
        }
        assert!(!p.is_protected(&e.sys_path("/Windows/Temp")));
        assert!(!p.is_protected(&e.sys_path("/Windows/Temp/a.tmp")));
    }

    #[test]
    fn exclude_set_semantics() {
        let (_d, e) = env();
        let home = e.home.to_string_lossy().into_owned();
        let set = ExcludeSet::from_patterns(
            &e,
            &[
                format!("{home}/keep"),
                format!("{home}/logs/*.log"),
                "~/.cache/foo?".to_string(),
            ],
        )
        .unwrap();
        assert!(set.is_excluded(&e.home.join("keep")));
        assert!(set.is_excluded(&e.home.join("keep/deep/file.txt")));
        assert!(!set.is_excluded(&e.home.join("keeper")));
        assert!(!set.is_excluded(&e.home.join("notkeep")));
        assert!(set.is_excluded(&e.home.join("logs/a.log")));
        assert!(!set.is_excluded(&e.home.join("logs/a.txt")));
        assert!(!set.is_excluded(&e.home.join("logs/sub/a.log")));
        assert!(set.is_excluded(&e.home.join(".cache/fooX/inner")));
        assert!(!set.is_excluded(&e.home.join(".cache/foo")));
        assert!(!ExcludeSet::empty().is_excluded(&e.home));
    }

    #[test]
    fn exclude_set_rejects_bad_patterns() {
        let (_d, e) = env();
        assert!(ExcludeSet::from_patterns(&e, &["relative/path"]).is_err());
        assert!(ExcludeSet::from_patterns(&e, &[""]).is_err());
        assert!(ExcludeSet::from_patterns(&e, &["/a/../b"]).is_err());
        assert!(ExcludeSet::from_patterns(&e, &["/a/[unclosed"]).is_err());
    }

    fn deleter(e: &Env, base: &Path, excl: &[&str]) -> SafeDeleter {
        SafeDeleter::new(e, base, ExcludeSet::from_patterns(e, excl).unwrap()).unwrap()
    }

    #[test]
    fn deleter_removes_inside_base_and_reports_bytes() {
        let (_d, e) = env();
        let base = e.home.join(".cache/app");
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("a.bin"), vec![0u8; 123]).unwrap();
        let d = deleter(&e, &base, &[]);
        assert_eq!(d.remove_file(&base.join("a.bin")).unwrap(), 123);
        assert!(!base.join("a.bin").exists());
    }

    #[test]
    fn deleter_refuses_outside_base_base_itself_and_lookalikes() {
        let (_d, e) = env();
        let base = e.home.join(".cache/app");
        fs::create_dir_all(&base).unwrap();
        let sibling = e.home.join(".cache/app2");
        fs::create_dir_all(&sibling).unwrap();
        fs::write(sibling.join("x"), "x").unwrap();
        fs::write(e.home.join("outside"), "x").unwrap();
        let d = deleter(&e, &base, &[]);
        for p in [
            sibling.join("x"),
            e.home.join("outside"),
            base.clone(),
            base.join("..").join("app2").join("x"),
        ] {
            let err = d.remove_file(&p).unwrap_err();
            assert!(
                matches!(err, SafeError::Refused(Refusal::OutsideBase, _)),
                "{p:?}: {err:?}"
            );
        }
        assert!(sibling.join("x").exists());
        assert!(e.home.join("outside").exists());
        assert!(d.remove_empty_dir(&base).is_err());
    }

    #[test]
    fn deleter_refuses_relative_paths() {
        let (_d, e) = env();
        let base = e.home.join("b");
        fs::create_dir_all(&base).unwrap();
        let d = deleter(&e, &base, &[]);
        assert!(matches!(
            d.remove_file(Path::new("x")).unwrap_err(),
            SafeError::Refused(Refusal::NotAbsolute, _)
        ));
    }

    #[test]
    fn symlink_leaf_is_removed_as_a_link_not_followed() {
        let (_d, e) = env();
        let base = e.home.join("b");
        fs::create_dir_all(&base).unwrap();
        let outside = e.home.join("precious.txt");
        fs::write(&outside, "keep me").unwrap();
        symlink(&outside, base.join("link")).unwrap();
        let d = deleter(&e, &base, &[]);
        d.remove_file(&base.join("link")).unwrap();
        assert!(fs::symlink_metadata(base.join("link")).is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep me");
    }

    #[test]
    fn symlinked_parent_escaping_the_base_is_refused() {
        let (_d, e) = env();
        let base = e.home.join("b");
        fs::create_dir_all(&base).unwrap();
        let outside_dir = e.home.join("elsewhere");
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(outside_dir.join("f"), "x").unwrap();
        symlink(&outside_dir, base.join("dirlink")).unwrap();
        let d = deleter(&e, &base, &[]);
        let err = d.remove_file(&base.join("dirlink").join("f")).unwrap_err();
        assert!(matches!(err, SafeError::Refused(Refusal::OutsideBase, _)));
        assert!(outside_dir.join("f").exists());
        // remove_file on a symlink-to-dir removes only the link
        d.remove_file(&base.join("dirlink")).unwrap();
        assert!(outside_dir.join("f").exists());
    }

    #[test]
    fn deleter_refuses_protected_and_excluded() {
        let (_d, e) = env();
        // A base that is itself protected is refused.
        assert!(SafeDeleter::new(&e, &e.home, ExcludeSet::empty()).is_err());
        assert!(SafeDeleter::new(&e, &e.home.join("Documents"), ExcludeSet::empty()).is_err());
        assert!(SafeDeleter::new(&e, Path::new("/"), ExcludeSet::empty()).is_err());
        // Protected item below an allowed base: ~/.ssh under a (silly) base of home's parent.
        let base = e.home.join("b");
        fs::create_dir_all(&base).unwrap();
        let keep = base.join("keep");
        fs::create_dir_all(&keep).unwrap();
        fs::write(keep.join("f"), "x").unwrap();
        fs::write(base.join("gone"), "x").unwrap();
        let d = deleter(&e, &base, &[keep.to_str().unwrap()]);
        assert!(d.remove_file(&keep.join("f")).unwrap_err().is_excluded());
        assert!(keep.join("f").exists());
        d.remove_file(&base.join("gone")).unwrap();
    }

    #[test]
    fn protected_item_inside_base_is_refused() {
        let (_d, e) = env();
        // base = home/.ssh's parent is home (protected), so use root/var/lib style:
        let base = e.sys_path("/var/lib");
        fs::create_dir_all(&base).unwrap();
        let d = deleter(&e, &base, &[]);
        // /var/lib is allowed as a base; its parent /var is protected (exact) but children are fine.
        fs::write(base.join("f"), "x").unwrap();
        d.remove_file(&base.join("f")).unwrap();
        // ssh keys are protected wherever the base is
        let ssh = e.home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        fs::write(ssh.join("id"), "k").unwrap();
        let d2 = SafeDeleter::new(&e, &ssh, ExcludeSet::empty());
        assert!(d2.is_err());
    }

    #[test]
    fn directories_are_not_removed_by_remove_file_and_only_when_empty() {
        let (_d, e) = env();
        let base = e.home.join("b");
        fs::create_dir_all(base.join("sub")).unwrap();
        fs::write(base.join("sub/f"), "x").unwrap();
        let d = deleter(&e, &base, &[]);
        assert!(matches!(
            d.remove_file(&base.join("sub")).unwrap_err(),
            SafeError::Refused(Refusal::IsDirectory, _)
        ));
        assert!(matches!(
            d.remove_empty_dir(&base.join("sub")).unwrap_err(),
            SafeError::Io(_)
        ));
        assert!(base.join("sub/f").exists());
        d.remove_file(&base.join("sub/f")).unwrap();
        d.remove_empty_dir(&base.join("sub")).unwrap();
        assert!(!base.join("sub").exists());
        assert!(matches!(
            d.remove_empty_dir(&base.join("nothere")).unwrap_err(),
            SafeError::Io(_)
        ));
    }
}
