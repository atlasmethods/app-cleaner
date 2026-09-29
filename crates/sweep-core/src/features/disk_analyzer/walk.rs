//! Filesystem helpers shared by the disk analyzer, the duplicate finder and the wiper:
//! file identity (hard links), device ids, hidden / system flags and the directories a
//! whole-disk walk must never enter.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ctx::{Env, Os};
use crate::safety::{is_within, normalize};

/// `(device, inode)` on Unix; `(volume serial, file index)` on Windows.
pub type FileId = (u64, u64);

/// Identity of an open file plus how many directory entries point at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    pub id: FileId,
    pub nlink: u64,
}

/// Identity from already-fetched metadata (free on Unix; `None` on Windows, where the
/// caller falls back to [`file_identity`]).
pub fn identity_from_meta(meta: &fs::Metadata) -> Option<FileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(FileIdentity {
            id: (meta.dev(), meta.ino()),
            nlink: meta.nlink(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

/// Identity of `path` (a regular file). On Windows this opens the file once.
pub fn file_identity(path: &Path, meta: &fs::Metadata) -> Option<FileIdentity> {
    if let Some(i) = identity_from_meta(meta) {
        return Some(i);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT,
        };
        let f = fs::OpenOptions::new()
            .access_mode(0)
            .share_mode(7)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .ok()?;
        // SAFETY: `info` is plain data and `f` outlives the call.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle() as _, &mut info) };
        if ok == 0 {
            return None;
        }
        let index = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
        return Some(FileIdentity {
            id: (info.dwVolumeSerialNumber as u64, index),
            nlink: info.nNumberOfLinks as u64,
        });
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

/// Device id of the filesystem holding `meta`'s file (Unix only).
pub fn dev_of(meta: &fs::Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(meta.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

/// May a walk that started on device `root_dev` descend into a directory on `dir_dev`?
/// When either is unknown (Windows) the answer is yes; Windows mount points are
/// reparse points, which walks never follow.
pub fn same_filesystem(root_dev: Option<u64>, dir_dev: Option<u64>) -> bool {
    match (root_dev, dir_dev) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

/// Modification time as Unix seconds (0 when unknown).
pub fn mtime_secs(meta: &fs::Metadata) -> i64 {
    match meta.modified() {
        Ok(t) => match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs() as i64,
            Err(e) => -(e.duration().as_secs() as i64),
        },
        Err(_) => 0,
    }
}

/// Hidden: dot-name on Unix, the hidden attribute on Windows, `UF_HIDDEN` or a dot-name
/// on macOS.
pub fn is_hidden(name: &std::ffi::OsStr, meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let _ = name;
        meta.file_attributes() & 0x2 != 0
    }
    #[cfg(not(windows))]
    {
        let dot = name.to_string_lossy().starts_with('.');
        #[cfg(target_os = "macos")]
        {
            use std::os::macos::fs::MetadataExt;
            return dot || meta.st_flags() & 0x8000 != 0;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = meta;
            dot
        }
    }
}

/// The Windows "system" file attribute (always false elsewhere; there the OS trees are
/// recognised by path, see `Protected::in_system_tree`).
pub fn has_system_attr(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x4 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = meta;
        false
    }
}

/// Directories a whole-disk walk never enters: pseudo filesystems whose "files" have
/// meaningless sizes (`/proc/kcore` reports 128 TB).
pub struct SkipDirs {
    dirs: Vec<PathBuf>,
}

impl SkipDirs {
    pub fn new(env: &Env) -> Self {
        let names: &[&str] = match env.os {
            Os::Linux => &["/proc", "/sys", "/dev"],
            Os::MacOs => &["/dev", "/System/Volumes/VM", "/private/var/vm"],
            Os::Windows => &[],
        };
        Self {
            dirs: names.iter().map(|d| env.sys_path(d)).collect(),
        }
    }

    /// Is `path` one of the skipped directories, or inside one?
    pub fn contains(&self, path: &Path) -> bool {
        self.dirs.iter().any(|d| is_within(path, d))
    }
}

/// Rate limiter for progress events shared by worker threads: `ready()` is true for at
/// most one caller per `every_ms` interval.
pub struct Throttle {
    start: std::time::Instant,
    last_ms: std::sync::atomic::AtomicU64,
    every_ms: u64,
}

impl Throttle {
    pub fn new(every_ms: u64) -> Self {
        Self {
            start: std::time::Instant::now(),
            last_ms: std::sync::atomic::AtomicU64::new(0),
            every_ms,
        }
    }

    pub fn ready(&self) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        let now = self.start.elapsed().as_millis() as u64;
        let last = self.last_ms.load(Relaxed);
        now.saturating_sub(last) >= self.every_ms
            && self
                .last_ms
                .compare_exchange(last, now, Relaxed, Relaxed)
                .is_ok()
    }
}

/// Lexically normalized absolute path, or an error message.
pub fn require_absolute(raw: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(raw);
    if !p.is_absolute() {
        return Err(format!("path must be absolute: {raw}"));
    }
    Ok(normalize(&p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_fs_decision() {
        assert!(same_filesystem(Some(1), Some(1)));
        assert!(!same_filesystem(Some(1), Some(2)));
        assert!(same_filesystem(None, Some(2)));
        assert!(same_filesystem(Some(1), None));
        assert!(same_filesystem(None, None));
    }

    #[test]
    fn skip_dirs_cover_pseudo_filesystems() {
        let env = Env::for_test(Path::new("/tmp/base"));
        let s = SkipDirs::new(&env);
        if env.os == Os::Linux {
            assert!(s.contains(&env.sys_path("/proc")));
            assert!(s.contains(&env.sys_path("/proc/1/mem")));
            assert!(s.contains(&env.sys_path("/sys/kernel")));
            assert!(s.contains(&env.sys_path("/dev/shm")));
            assert!(!s.contains(&env.sys_path("/home")));
            assert!(!s.contains(&env.sys_path("/devices")));
        }
    }

    #[cfg(unix)]
    #[test]
    fn hidden_and_identity() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join(".secret");
        let b = d.path().join("plain");
        fs::write(&a, b"x").unwrap();
        fs::hard_link(&a, &b).unwrap();
        let ma = fs::metadata(&a).unwrap();
        assert!(is_hidden(a.file_name().unwrap(), &ma));
        assert!(!is_hidden(b.file_name().unwrap(), &ma));
        let ia = file_identity(&a, &ma).unwrap();
        let ib = file_identity(&b, &fs::metadata(&b).unwrap()).unwrap();
        assert_eq!(ia.id, ib.id);
        assert_eq!(ia.nlink, 2);
    }
}
