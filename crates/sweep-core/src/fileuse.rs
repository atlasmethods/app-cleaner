//! "Is this file still in use?" knowledge behind a trait, so the temp-file cleaner never
//! removes something a running program depends on, and so tests can fake the evidence.
//!
//! Two kinds of evidence are used:
//!
//! * **Activity timestamps.** A file is only idle when its modification time, access time and
//!   status-change time (Unix `ctime`; creation time on Windows) are ALL older than the
//!   threshold, the same rule `systemd-tmpfiles` applies. Directories ignore the access time:
//!   merely listing a directory (which every scan does) updates it.
//! * **Open files (Linux only).** The `(device, inode)` pairs of every file that any process
//!   currently has open, mapped into memory, as its working directory or as its executable,
//!   read from `/proc`. Windows refuses to delete open files on its own, and macOS has no
//!   `/proc`, so this guard is a no-op there.
//!
//! The `/proc` location is resolved through [`Env::sys_path`], so a test can place a fake
//! `/proc` under the fixture root.

use std::collections::HashSet;
use std::fs::Metadata;
use std::time::{Duration, SystemTime};

use crate::ctx::Env;

/// `(device, inode)` pairs of files in use.
#[derive(Debug, Default, Clone)]
pub struct OpenFiles {
    ids: HashSet<(u64, u64)>,
}

impl OpenFiles {
    pub fn new(ids: HashSet<(u64, u64)>) -> Self {
        Self { ids }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Is the file (or directory) described by `meta` in use by some process?
    pub fn contains(&self, meta: &Metadata) -> bool {
        !self.ids.is_empty() && file_id(meta).is_some_and(|id| self.ids.contains(&id))
    }
}

#[cfg(unix)]
fn file_id(meta: &Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id(_meta: &Metadata) -> Option<(u64, u64)> {
    None
}

/// Source of in-use evidence for the cleaner.
pub trait FileUse: Send + Sync {
    /// Files currently open by any process (empty where unsupported or unreadable).
    fn open_files(&self, env: &Env) -> OpenFiles;

    /// The newest of the timestamps that show the entry is alive. `None` when even the
    /// modification time cannot be read (treated as "in use").
    fn last_activity(&self, meta: &Metadata, is_dir: bool) -> Option<SystemTime>;
}

/// The real thing.
#[derive(Debug, Clone, Copy)]
pub struct SystemFileUse {
    /// Ignore the status-change time. Only the test fixtures need this: they can set mtime and
    /// atime but not ctime, which is always "now" for a file created a moment ago.
    pub ignore_ctime: bool,
}

impl Default for SystemFileUse {
    fn default() -> Self {
        Self {
            ignore_ctime: default_ignore_ctime(),
        }
    }
}

/// Unit tests of this crate and (with the `testutil` feature) sandboxed end-to-end runs that
/// set `CLEARSWEEP_TEST_IGNORE_CTIME` ignore the status-change time. A release build never does.
fn default_ignore_ctime() -> bool {
    if cfg!(test) {
        return true;
    }
    #[cfg(feature = "testutil")]
    {
        if std::env::var_os("CLEARSWEEP_TEST_IGNORE_CTIME").is_some_and(|v| !v.is_empty()) {
            return true;
        }
    }
    false
}

impl FileUse for SystemFileUse {
    fn open_files(&self, env: &Env) -> OpenFiles {
        #[cfg(target_os = "linux")]
        {
            OpenFiles::new(linux::collect(&env.sys_path("/proc")))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = env;
            OpenFiles::default()
        }
    }

    fn last_activity(&self, meta: &Metadata, is_dir: bool) -> Option<SystemTime> {
        last_activity(meta, is_dir, !self.ignore_ctime)
    }
}

/// Newest of mtime, atime (files only) and ctime (Unix) / creation time (Windows).
pub fn last_activity(meta: &Metadata, is_dir: bool, use_ctime: bool) -> Option<SystemTime> {
    let mut newest = meta.modified().ok()?;
    if !is_dir {
        if let Ok(a) = meta.accessed() {
            newest = newest.max(a);
        }
    }
    if use_ctime {
        if let Some(c) = change_time(meta) {
            newest = newest.max(c);
        }
    }
    Some(newest)
}

#[cfg(unix)]
fn change_time(meta: &Metadata) -> Option<SystemTime> {
    use std::os::unix::fs::MetadataExt;
    let secs = meta.ctime();
    let nanos = u32::try_from(meta.ctime_nsec()).unwrap_or(0);
    if secs >= 0 {
        Some(SystemTime::UNIX_EPOCH + Duration::new(secs as u64, nanos))
    } else {
        None
    }
}

#[cfg(not(unix))]
fn change_time(meta: &Metadata) -> Option<SystemTime> {
    meta.created().ok()
}

/// Has the entry been quiet for at least `age` at `now`? A future-dated or unreadable
/// timestamp counts as active.
pub fn is_idle(activity: Option<SystemTime>, age: Duration, now: SystemTime) -> bool {
    match activity.and_then(|t| now.duration_since(t).ok()) {
        Some(d) => d >= age,
        None => false,
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashSet;
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    fn add(ids: &mut HashSet<(u64, u64)>, path: &Path) {
        // `metadata` follows the /proc magic symlink to the real (even deleted) file.
        if let Ok(m) = fs::metadata(path) {
            if m.is_file() || m.is_dir() {
                ids.insert((m.dev(), m.ino()));
            }
        }
    }

    /// Path column of a `/proc/PID/maps` line: `addr perms offset dev inode   path`.
    fn maps_path(line: &str) -> Option<&str> {
        let mut rest = line;
        for _ in 0..5 {
            rest = rest.trim_start();
            let end = rest.find(char::is_whitespace)?;
            rest = &rest[end..];
        }
        let p = rest.trim();
        p.starts_with('/').then_some(p)
    }

    pub(super) fn collect(proc_root: &Path) -> HashSet<(u64, u64)> {
        let mut ids = HashSet::new();
        let mut mapped: HashSet<String> = HashSet::new();
        let Ok(rd) = fs::read_dir(proc_root) else {
            return ids;
        };
        for entry in rd.flatten() {
            let name = entry.file_name();
            let is_pid = name
                .to_str()
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
            if !is_pid {
                continue;
            }
            let dir = entry.path();
            // Permission errors (other users' processes) are ignored throughout.
            if let Ok(fds) = fs::read_dir(dir.join("fd")) {
                for fd in fds.flatten() {
                    add(&mut ids, &fd.path());
                }
            }
            add(&mut ids, &dir.join("cwd"));
            add(&mut ids, &dir.join("exe"));
            if let Ok(maps) = fs::read_to_string(dir.join("maps")) {
                for line in maps.lines() {
                    if let Some(p) = maps_path(line) {
                        if !p.ends_with(" (deleted)") {
                            mapped.insert(p.to_string());
                        }
                    }
                }
            }
        }
        for p in mapped {
            add(&mut ids, Path::new(&p));
        }
        ids
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_maps_lines() {
            let l = "7f3c2a000000-7f3c2a021000 r--p 00000000 08:01 1234567   /tmp/my dir/lib x.so";
            assert_eq!(maps_path(l), Some("/tmp/my dir/lib x.so"));
            assert_eq!(
                maps_path("7ffd1c000000-7ffd1c021000 rw-p 00000000 00:00 0   [stack]"),
                None
            );
            assert_eq!(
                maps_path("7f3c2a000000-7f3c2a021000 rw-p 00000000 00:00 0"),
                None
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_needs_old_and_not_future() {
        let now = SystemTime::now();
        let h = |n: u64| Duration::from_secs(n * 3600);
        assert!(is_idle(Some(now - h(25)), h(24), now));
        assert!(!is_idle(Some(now - h(1)), h(24), now));
        assert!(!is_idle(Some(now + h(1)), h(24), now));
        assert!(!is_idle(None, h(24), now));
        assert!(is_idle(Some(now), Duration::ZERO, now));
    }
}
