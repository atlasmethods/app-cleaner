//! Free-space wiping: fill the free space of a volume with pattern data so previously
//! deleted files cannot be recovered, then give the space back.
//!
//! Safety properties (a full disk left behind is a critical bug):
//! * everything is written inside one hidden directory `.clearsweep-wipe-<16 hex>` that
//!   this module created itself;
//! * the directory is owned by a [`WipeGuard`] whose `Drop` removes it, so an error, a
//!   cancellation or a panic all give the space back;
//! * a run that was killed outright leaves a stale directory behind; the next run (and
//!   [`remove_stale`]) removes such directories, recognised by the exact name pattern;
//! * removing the filler files deliberately ignores the user's exclusion list (an
//!   exclusion covering the mount would otherwise keep the disk full) and never uses
//!   secure deletion (the files hold nothing of value). Removal still goes through
//!   `SafeDeleter`; only if that refuses does it fall back to a plain unlink of the
//!   files this module created.
//!
//! Not implemented: filling the file-system metadata (MFT records / inodes) with small
//! files. Only file data blocks are overwritten.

use rand::rngs::StdRng;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::ctx::Ctx;
use crate::error::{ApiError, Result};
use crate::features::secure_delete::{fill_chunk, Pattern};
use crate::fsutil::random_id;
use crate::job::{Job, ProgressEvent};
use crate::safety::{io_message, SafeDeleter, Safety};

pub const WIPE_PREFIX: &str = ".clearsweep-wipe-";
/// Largest single filler file.
pub const MAX_FILE_BYTES: u64 = 1 << 30;
const CHUNK: usize = 1 << 20;

// ---------------------------------------------------------------- filesystem abstraction

/// The two operations whose behaviour tests replace: measuring free space and writing.
pub trait WipeFs: Send + Sync {
    /// Bytes an unprivileged writer can still allocate on the filesystem holding `dir`.
    fn free_space(&self, dir: &Path) -> io::Result<u64>;
    fn write(&self, file: &mut File, data: &[u8]) -> io::Result<()> {
        file.write_all(data)
    }
}

/// The real thing.
pub struct RealFs;

impl WipeFs for RealFs {
    fn free_space(&self, dir: &Path) -> io::Result<u64> {
        free_space_of(dir)
    }
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
pub fn free_space_of(dir: &Path) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL"))?;
    // SAFETY: `st` is plain data written by statvfs; `c` is a valid C string.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

#[cfg(windows)]
pub fn free_space_of(dir: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: all pointers are valid for the duration of the call.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut free) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(avail)
}

/// The error a full disk produces (ENOSPC / EDQUOT / ERROR_DISK_FULL).
pub fn is_disk_full(e: &io::Error) -> bool {
    #[cfg(unix)]
    {
        matches!(e.raw_os_error(), Some(c) if c == libc::ENOSPC || c == libc::EDQUOT)
    }
    #[cfg(windows)]
    {
        // ERROR_HANDLE_DISK_FULL, ERROR_DISK_FULL
        matches!(e.raw_os_error(), Some(39) | Some(112))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = e;
        false
    }
}

/// An `io::Error` that [`is_disk_full`] recognises (for simulations).
pub fn disk_full_error() -> io::Error {
    #[cfg(unix)]
    {
        io::Error::from_raw_os_error(libc::ENOSPC)
    }
    #[cfg(not(unix))]
    {
        io::Error::from_raw_os_error(112)
    }
}

// ---------------------------------------------------------------- location

/// Do two paths live on the same filesystem?
pub fn same_volume(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(a), fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        use std::path::Component;
        let prefix = |p: &Path| {
            p.components().next().and_then(|c| match c {
                Component::Prefix(x) => Some(x.as_os_str().to_string_lossy().to_lowercase()),
                _ => None,
            })
        };
        matches!((prefix(a), prefix(b)), (Some(x), Some(y)) if x == y)
    }
}

/// Places where a wipe directory may be created for `mount`, best first: the mount
/// itself, then the user's home and temp folders when they live on the same volume.
pub fn candidate_locations(ctx: &Ctx, mount: &Path) -> Vec<PathBuf> {
    let mut v = vec![mount.to_path_buf()];
    for extra in [&ctx.env.home, &ctx.env.temp_dir] {
        if extra.is_dir() && !v.contains(extra) && same_volume(extra, mount) {
            v.push(extra.clone());
        }
    }
    v
}

fn is_wipe_dir_name(name: &str) -> bool {
    name.strip_prefix(WIPE_PREFIX)
        .is_some_and(|h| h.len() == 16 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// Remove a wipe directory (and the filler files in it). Returns an error message when
/// something could not be removed.
fn remove_wipe_dir(dir: &Path, safety: &Arc<Safety>) -> std::result::Result<(), String> {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
    if !name.as_deref().is_some_and(is_wipe_dir_name) {
        return Err(format!(
            "refusing to remove {}: not a wipe folder",
            dir.display()
        ));
    }
    match fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_dir() => {}
        Ok(_) => return Err(format!("{} is not a plain folder", dir.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_message(&e)),
    }
    let parent = dir.parent().unwrap_or(dir);
    // No exclusions, no secure deletion (see the module docs).
    let deleter = SafeDeleter::for_selection(safety.clone(), parent).ok();
    let mut first_error: Option<String> = None;
    for ent in fs::read_dir(dir).map_err(|e| io_message(&e))?.flatten() {
        let p = ent.path();
        if ent.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue; // never recurse into something we did not create
        }
        let via_safe = deleter.as_ref().map(|d| d.remove_file(&p).is_ok());
        if via_safe != Some(true) && p.exists() {
            if let Err(e) = fs::remove_file(&p) {
                first_error.get_or_insert(format!("{}: {}", p.display(), io_message(&e)));
            }
        }
    }
    match fs::remove_dir(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            Err(first_error.unwrap_or_else(|| format!("{}: {}", dir.display(), io_message(&e))))
        }
    }
}

/// Owns a wipe directory; removes it when dropped.
pub struct WipeGuard {
    dir: PathBuf,
    done: bool,
    safety: Arc<Safety>,
}

impl WipeGuard {
    /// Create a fresh wipe directory in the first writable `candidates` entry that lies
    /// on the same volume as `mount`.
    pub fn create(candidates: &[PathBuf], mount: &Path, safety: &Arc<Safety>) -> Result<WipeGuard> {
        let mut last: Option<io::Error> = None;
        for c in candidates {
            if c != mount && !same_volume(c, mount) {
                continue;
            }
            let dir = c.join(format!("{WIPE_PREFIX}{}", random_id()));
            match fs::create_dir(&dir) {
                Ok(()) => {
                    return Ok(WipeGuard {
                        dir,
                        done: false,
                        safety: safety.clone(),
                    })
                }
                Err(e) => last = Some(e),
            }
        }
        Err(ApiError::permission_denied(format!(
            "cannot write to {}{}; run ClearSweep with administrator rights to wipe this drive",
            mount.display(),
            last.map(|e| format!(" ({})", io_message(&e)))
                .unwrap_or_default()
        )))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Remove the directory now, reporting failure (dropping retries silently).
    pub fn cleanup(&mut self) -> std::result::Result<(), String> {
        if self.done {
            return Ok(());
        }
        let r = remove_wipe_dir(&self.dir, &self.safety);
        if r.is_ok() {
            self.done = true;
        }
        r
    }
}

impl Drop for WipeGuard {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Remove `.clearsweep-wipe-*` directories left behind by an earlier crashed run in
/// `dir`. Returns the directories removed.
pub fn remove_stale(dir: &Path, safety: &Arc<Safety>) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return removed;
    };
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().into_owned();
        if is_wipe_dir_name(&name) && ent.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let p = ent.path();
            if remove_wipe_dir(&p, safety).is_ok() {
                removed.push(p);
            }
        }
    }
    removed
}

// ---------------------------------------------------------------- the wipe

#[derive(Debug, Clone)]
pub struct FillFile {
    pub path: PathBuf,
    pub len: u64,
}

/// Called after every completed pass (files synced), before the next one starts.
pub struct PassDone<'a> {
    pub dir: &'a Path,
    pub pass: usize,
    pub pattern: &'a Pattern,
    pub files: &'a [FillFile],
}

pub struct FillParams<'a> {
    pub mount: &'a Path,
    /// Built with an EMPTY exclusion list (see the module docs).
    pub safety: Arc<Safety>,
    pub candidates: Vec<PathBuf>,
    pub patterns: Vec<Pattern>,
    pub fs: &'a dyn WipeFs,
    pub job: &'a Job,
    pub max_file: u64,
    pub on_pass_done: Option<&'a dyn Fn(&PassDone<'_>)>,
}

#[derive(Debug, Clone)]
pub struct FillReport {
    pub files: usize,
    /// Bytes written over all passes.
    pub bytes_written: u64,
    /// Bytes of a single pass (= the free space that was filled).
    pub bytes_per_pass: u64,
    pub free_before: Option<u64>,
    pub free_after: Option<u64>,
    pub location: PathBuf,
    pub stale_removed: usize,
    pub duration_ms: u64,
}

struct Progress<'a> {
    job: &'a Job,
    total: Option<u64>,
    done: u64,
    last: Instant,
    passes: usize,
}

impl Progress<'_> {
    fn add(&mut self, n: u64, pass: usize) {
        self.done += n;
        if self.last.elapsed().as_millis() < 100 {
            return;
        }
        self.last = Instant::now();
        self.emit(pass);
    }

    fn emit(&self, pass: usize) {
        let mut ev = ProgressEvent::new("wipe").message(format!(
            "Pass {} of {}: {} written",
            pass + 1,
            self.passes,
            human(self.done)
        ));
        if let Some(t) = self.total.filter(|t| *t > 0) {
            ev = ev.fraction((self.done as f64 / t as f64).min(0.999));
        }
        ev.current = Some(self.done);
        ev.total = self.total;
        self.job.progress(ev);
    }
}

fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}

/// Fill the free space of `params.mount`'s volume once per pattern, then remove
/// everything again. Always removes the wipe directory, also on `Err`.
pub fn fill_free_space(params: &FillParams<'_>) -> Result<FillReport> {
    let start = Instant::now();
    let job = params.job;
    if params.patterns.is_empty() {
        return Err(ApiError::invalid_params("no overwrite pattern"));
    }
    job.check_cancelled()?;

    // Leftovers of an earlier crashed run would otherwise eat the space we measure.
    let mut stale = 0;
    for c in &params.candidates {
        stale += remove_stale(c, &params.safety).len();
    }

    let mut guard = WipeGuard::create(&params.candidates, params.mount, &params.safety)?;
    let location = guard
        .dir()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| params.mount.to_path_buf());
    let free_before = params.fs.free_space(guard.dir()).ok();
    let passes = params.patterns.len();
    let mut progress = Progress {
        job,
        total: free_before.map(|f| f.saturating_mul(passes as u64)),
        done: 0,
        last: Instant::now(),
        passes,
    };

    let run = (|| -> Result<Vec<FillFile>> {
        let mut rng: StdRng = rand::make_rng();
        let mut buf = vec![0u8; CHUNK];
        let dir = guard.dir().to_path_buf();
        let files = fill_pass(
            params,
            &dir,
            &params.patterns[0],
            &mut rng,
            &mut buf,
            &mut progress,
        )?;
        if let Some(cb) = params.on_pass_done {
            cb(&PassDone {
                dir: &dir,
                pass: 0,
                pattern: &params.patterns[0],
                files: &files,
            });
        }
        for (i, pat) in params.patterns.iter().enumerate().skip(1) {
            rewrite_pass(params, &files, pat, i, &mut rng, &mut buf, &mut progress)?;
            if let Some(cb) = params.on_pass_done {
                cb(&PassDone {
                    dir: &dir,
                    pass: i,
                    pattern: pat,
                    files: &files,
                });
            }
        }
        Ok(files)
    })();

    // Give the space back before reporting anything.
    let cleaned = guard.cleanup();
    let files = run?;
    cleaned.map_err(|e| {
        ApiError::io(format!(
            "the wipe finished but its temporary files could not all be removed ({e}); delete the folder {} manually",
            guard.dir().display()
        ))
    })?;
    let bytes_per_pass: u64 = files.iter().map(|f| f.len).sum();
    Ok(FillReport {
        files: files.len(),
        bytes_written: progress.done,
        bytes_per_pass,
        free_before,
        free_after: params.fs.free_space(&location).ok(),
        location,
        stale_removed: stale,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Pass 1: create files until the disk is full.
fn fill_pass(
    params: &FillParams<'_>,
    dir: &Path,
    pat: &Pattern,
    rng: &mut StdRng,
    buf: &mut [u8],
    progress: &mut Progress<'_>,
) -> Result<Vec<FillFile>> {
    let job = params.job;
    let mut files: Vec<FillFile> = Vec::new();
    progress.emit(0);
    loop {
        job.check_cancelled()?;
        let path = dir.join(format!("fill-{:06}.bin", files.len()));
        let mut f = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => f,
            Err(e) if is_disk_full(&e) => break,
            Err(e) => return Err(io_err(&path, &e)),
        };
        let mut in_file = 0u64;
        let mut full = false;
        while in_file < params.max_file {
            job.check_cancelled()?;
            let n = ((params.max_file - in_file).min(buf.len() as u64)) as usize;
            fill_chunk(&mut buf[..n], pat, in_file, rng);
            match params.fs.write(&mut f, &buf[..n]) {
                Ok(()) => {
                    in_file += n as u64;
                    progress.add(n as u64, 0);
                }
                Err(e) if is_disk_full(&e) => {
                    full = true;
                    break;
                }
                Err(e) => return Err(io_err(&path, &e)),
            }
        }
        let synced = f.flush().and_then(|_| f.sync_all());
        match synced {
            Ok(()) => {}
            Err(e) if full && is_disk_full(&e) => {}
            Err(e) => return Err(io_err(&path, &e)),
        }
        let len = f.metadata().map(|m| m.len()).unwrap_or(in_file);
        // A write that hit the end of the disk may have stored part of its chunk.
        progress.done += len.saturating_sub(in_file);
        drop(f);
        files.push(FillFile { path, len });
        if full {
            break;
        }
    }
    Ok(files)
}

/// Passes 2..n: overwrite the same files in place with the next pattern.
fn rewrite_pass(
    params: &FillParams<'_>,
    files: &[FillFile],
    pat: &Pattern,
    pass: usize,
    rng: &mut StdRng,
    buf: &mut [u8],
    progress: &mut Progress<'_>,
) -> Result<()> {
    let job = params.job;
    progress.emit(pass);
    for ff in files {
        job.check_cancelled()?;
        let mut f = OpenOptions::new()
            .write(true)
            .open(&ff.path)
            .map_err(|e| io_err(&ff.path, &e))?;
        f.seek(SeekFrom::Start(0))
            .map_err(|e| io_err(&ff.path, &e))?;
        let mut done = 0u64;
        while done < ff.len {
            job.check_cancelled()?;
            let n = ((ff.len - done).min(buf.len() as u64)) as usize;
            fill_chunk(&mut buf[..n], pat, done, rng);
            params
                .fs
                .write(&mut f, &buf[..n])
                .map_err(|e| io_err(&ff.path, &e))?;
            done += n as u64;
            progress.add(n as u64, pass);
        }
        f.flush()
            .and_then(|_| f.sync_all())
            .map_err(|e| io_err(&ff.path, &e))?;
    }
    Ok(())
}

fn io_err(path: &Path, e: &io::Error) -> ApiError {
    ApiError::from(io::Error::new(
        e.kind(),
        format!("{}: {}", path.display(), io_message(e)),
    ))
}
