//! Execution context: where the filesystem roots are and how commands run.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::runner::{CommandRunner, MockRunner, SystemRunner};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Os {
    Linux,
    Windows,
    MacOs,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

/// Filesystem environment. Every system-wide path must be resolved through
/// [`Env::sys_path`] so tests (and `CLEARSWEEP_ROOT`) can redirect it safely.
#[derive(Debug, Clone)]
pub struct Env {
    /// `/` normally; prefix for ALL system-wide paths (`/tmp`, `/var/cache`, `/etc`...).
    pub root: PathBuf,
    pub home: PathBuf,
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// App data: settings.json, backups, history.
    pub data_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub os: Os,
}

fn default_root() -> PathBuf {
    if cfg!(windows) {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        PathBuf::from(format!("{drive}\\"))
    } else {
        PathBuf::from("/")
    }
}

impl Env {
    /// Real directories; honors `CLEARSWEEP_ROOT` and `CLEARSWEEP_DATA_DIR`
    /// (HOME / XDG_* are honored by the `dirs` crate).
    pub fn detect() -> Env {
        let root = std::env::var_os("CLEARSWEEP_ROOT")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(default_root);
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let config_dir = dirs::config_dir().unwrap_or_else(|| home.join(".config"));
        let cache_dir = dirs::cache_dir().unwrap_or_else(|| home.join(".cache"));
        let data_dir = std::env::var_os("CLEARSWEEP_DATA_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .unwrap_or_else(|| home.join(".local/share"))
                    .join("clearsweep")
            });
        let temp_dir = std::env::temp_dir();
        Env {
            root,
            home,
            config_dir,
            cache_dir,
            data_dir,
            temp_dir,
            os: Os::current(),
        }
    }

    /// Everything under `base` (typically a temp dir). Never touches real user paths.
    pub fn for_test(base: &Path) -> Env {
        let home = base.join("home");
        Env {
            root: base.join("root"),
            config_dir: home.join(".config"),
            cache_dir: home.join(".cache"),
            data_dir: base.join("data"),
            temp_dir: base.join("root").join("tmp"),
            home,
            os: Os::current(),
        }
    }

    /// Join an absolute system path under `root`: `sys_path("/var/cache/apt")`.
    /// Leading separators / drive prefixes are stripped so the result can never
    /// escape the root by being absolute.
    pub fn sys_path(&self, abs: impl AsRef<Path>) -> PathBuf {
        let mut out = self.root.clone();
        for c in abs.as_ref().components() {
            match c {
                Component::Normal(p) => out.push(p),
                Component::ParentDir => out.push(".."),
                Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            }
        }
        out
    }
}

/// Everything a handler needs: environment paths plus a command runner.
#[derive(Clone)]
pub struct Ctx {
    pub env: Env,
    pub runner: Arc<dyn CommandRunner>,
}

impl Ctx {
    pub fn new(env: Env, runner: Arc<dyn CommandRunner>) -> Self {
        Self { env, runner }
    }
    pub fn system() -> Self {
        Self::new(Env::detect(), Arc::new(SystemRunner))
    }
    /// Test context rooted in `tempdir`, running commands through `runner`.
    pub fn test(tempdir: &Path, runner: MockRunner) -> Self {
        Self::new(Env::for_test(tempdir), Arc::new(runner))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_test_is_fully_under_base() {
        let base = Path::new("/tmp/base");
        let e = Env::for_test(base);
        for p in [
            &e.root,
            &e.home,
            &e.config_dir,
            &e.cache_dir,
            &e.data_dir,
            &e.temp_dir,
        ] {
            assert!(p.starts_with(base), "{p:?}");
        }
    }

    #[test]
    fn sys_path_joins_under_root() {
        let e = Env::for_test(Path::new("/tmp/base"));
        assert_eq!(
            e.sys_path("/var/cache/apt"),
            Path::new("/tmp/base/root/var/cache/apt")
        );
        assert_eq!(
            e.sys_path("etc/hosts"),
            Path::new("/tmp/base/root/etc/hosts")
        );
        assert_eq!(e.sys_path("/"), e.root);
    }

    #[test]
    fn ctx_test_uses_mock_runner() {
        let m = MockRunner::new();
        let ctx = Ctx::test(Path::new("/tmp/x"), m.clone());
        let _ = ctx.runner.run("echo", &["hi"]);
        assert_eq!(m.calls().len(), 1);
    }
}
