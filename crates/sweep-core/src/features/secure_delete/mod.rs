//! Secure delete: irrecoverably overwrite and remove files.
//!
//! Methods:
//! - `secure_delete.delete { paths: [..], passes? }` -> `{ results: [{path, ok, error?, bytes}], totalBytes }`
//!
//! Overwriting cannot be guaranteed on SSDs, journaling/copy-on-write filesystems or
//! snapshotted volumes; it is best effort and documented as such in the UI.

use rand::rngs::StdRng;
use rand::{Rng, RngExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use walkdir::WalkDir;

use crate::api::Registry;
use crate::ctx::Ctx;
use crate::error::{ApiError, Result};
use crate::features::settings;
use crate::job::{Job, ProgressEvent};
use crate::safety::{self, io_message, ExcludeSet, SafeDeleter, Safety};

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["secure_delete.delete"];

pub fn register(r: &mut Registry) {
    r.add("secure_delete.delete", delete);
}

/// Overwrite chunk size.
const CHUNK: usize = 1024 * 1024;

/// One overwrite pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// The same byte everywhere.
    Byte(u8),
    /// A byte sequence repeated across the whole file (phase continues across chunks).
    Bytes(Vec<u8>),
    /// Fresh CSPRNG output.
    Random,
}

/// The overwrite schedule for a supported pass count (1, 3, 7 or 35). Any other value
/// yields a single random pass; callers validate the count beforehand.
pub fn overwrite_passes(passes: u32) -> Vec<Pattern> {
    match passes {
        3 => vec![Pattern::Byte(0x00), Pattern::Byte(0xFF), Pattern::Random], // DoD 5220.22-M
        7 => {
            let mut v = vec![Pattern::Byte(0xFF), Pattern::Byte(0x00)];
            v.extend(std::iter::repeat_n(Pattern::Random, 5));
            v
        }
        35 => gutmann_patterns(),
        _ => vec![Pattern::Random],
    }
}

/// The 35-pass Gutmann schedule: 4 random, the 27 fixed patterns from the published
/// table (passes 5-31, in order), 4 random.
pub fn gutmann_patterns() -> Vec<Pattern> {
    let b = |v: &[u8]| Pattern::Bytes(v.to_vec());
    let mut v = Vec::with_capacity(35);
    v.extend(std::iter::repeat_n(Pattern::Random, 4));
    v.extend([
        b(&[0x55, 0x55, 0x55]), // 5
        b(&[0xAA, 0xAA, 0xAA]), // 6
        b(&[0x92, 0x49, 0x24]), // 7
        b(&[0x49, 0x24, 0x92]), // 8
        b(&[0x24, 0x92, 0x49]), // 9
    ]);
    for byte in 0..=0xFu8 {
        v.push(Pattern::Byte(byte * 0x11)); // 10-25: 00, 11, 22, ... FF
    }
    v.extend([
        b(&[0x92, 0x49, 0x24]), // 26
        b(&[0x49, 0x24, 0x92]), // 27
        b(&[0x24, 0x92, 0x49]), // 28
        b(&[0x6D, 0xB6, 0xDB]), // 29
        b(&[0xB6, 0xDB, 0x6D]), // 30
        b(&[0xDB, 0x6D, 0xB6]), // 31
    ]);
    v.extend(std::iter::repeat_n(Pattern::Random, 4));
    v
}

fn fill_chunk(buf: &mut [u8], pat: &Pattern, offset: u64, rng: &mut StdRng) {
    match pat {
        Pattern::Byte(b) => buf.fill(*b),
        Pattern::Bytes(seq) => {
            let n = seq.len() as u64;
            for (i, out) in buf.iter_mut().enumerate() {
                *out = seq[((offset + i as u64) % n) as usize];
            }
        }
        Pattern::Random => rng.fill_bytes(buf),
    }
}

/// Open an existing regular file for read+write without following symlinks.
fn open_rw_nofollow(path: &Path) -> io::Result<File> {
    let lm = fs::symlink_metadata(path)?;
    if !lm.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let open = || {
        let mut o = OpenOptions::new();
        o.read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_FLAG_OPEN_REPARSE_POINT: open a link itself instead of its target.
            o.custom_flags(0x0020_0000);
        }
        o.open(path)
    };
    let f = match open() {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            // A read-only file we own can still be unlinked, so make it writable first.
            let mut perm = lm.permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                perm.set_mode(perm.mode() | 0o200);
            }
            #[cfg(not(unix))]
            {
                #[allow(clippy::permissions_set_readonly_false)]
                perm.set_readonly(false);
            }
            fs::set_permissions(path, perm)?;
            open()?
        }
        other => other?,
    };
    if !f.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(f)
}

/// Write one full pass over `len` bytes, then fsync.
fn write_pass(
    f: &mut File,
    len: u64,
    pat: &Pattern,
    rng: &mut StdRng,
    mut on_bytes: impl FnMut(u64),
) -> io::Result<()> {
    f.seek(SeekFrom::Start(0))?;
    let mut buf = vec![0u8; CHUNK.min(len.max(1) as usize)];
    let mut done = 0u64;
    while done < len {
        let n = (len - done).min(buf.len() as u64) as usize;
        fill_chunk(&mut buf[..n], pat, done, rng);
        f.write_all(&buf[..n])?;
        done += n as u64;
        on_bytes(done);
    }
    f.flush()?;
    f.sync_all()
}

/// Overwrite the whole content of a regular file with a single `pattern` pass (no
/// rename/unlink). Symlinks and non-regular files are refused. Returns the length.
pub fn overwrite_file_in_place(path: &Path, pattern: &Pattern) -> io::Result<u64> {
    let mut f = open_rw_nofollow(path)?;
    let len = f.metadata()?.len();
    let mut rng: StdRng = rand::make_rng();
    write_pass(&mut f, len, pattern, &mut rng, |_| {})?;
    Ok(len)
}

fn random_name(len: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng: StdRng = rand::make_rng();
    (0..len.max(1))
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect()
}

/// Number of directory entries (names) pointing at this file; 1 where it cannot be told.
pub fn link_count(meta: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.nlink()
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        1
    }
}

/// Overwrite `path` with `passes` passes (each fsynced), rename it to a random name of
/// the same length, truncate to zero and unlink. Refuses symlinks and non-regular
/// files. Returns the number of bytes that were overwritten.
///
/// Cancellation is honoured between passes (the file then stays, partially overwritten).
pub fn secure_delete_file(path: &Path, passes: u32, job: &Job) -> Result<u64> {
    let mut f = open_rw_nofollow(path).map_err(|e| {
        ApiError::from(io::Error::new(
            e.kind(),
            format!("{}: {}", path.display(), io_message(&e)),
        ))
    })?;
    let meta = f.metadata()?;
    if link_count(&meta) > 1 {
        // Overwriting would destroy the content behind the file's other names too.
        return Err(ApiError::invalid_params(format!(
            "{}: the file has {} hard links; overwriting it would change the other names as well",
            path.display(),
            link_count(&meta)
        )));
    }
    let len = meta.len();
    let patterns = overwrite_passes(passes);
    let total_passes = patterns.len();
    let mut rng: StdRng = rand::make_rng();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for (i, pat) in patterns.iter().enumerate() {
        job.check_cancelled()?;
        let mut chunks = 0u64;
        write_pass(&mut f, len, pat, &mut rng, |_| {
            chunks += 1;
            if chunks & 15 == 0 {
                job.progress(
                    ProgressEvent::new("secure_delete")
                        .fraction(i as f64 / total_passes as f64)
                        .message(format!("{name}: pass {} of {total_passes}", i + 1)),
                );
            }
        })?;
        job.progress(
            ProgressEvent::new("secure_delete")
                .fraction((i + 1) as f64 / total_passes as f64)
                .message(format!("{name}: pass {} of {total_passes}", i + 1)),
        );
    }

    // Hide the original name, then drop the content before unlinking.
    let mut current: PathBuf = path.to_path_buf();
    if let Some(parent) = path.parent() {
        for _ in 0..8 {
            let cand = parent.join(random_name(name.chars().count()));
            if fs::symlink_metadata(&cand).is_ok() {
                continue;
            }
            if fs::rename(&current, &cand).is_ok() {
                current = cand;
            }
            break;
        }
    }
    f.set_len(0)?;
    f.sync_all()?;
    drop(f);
    fs::remove_file(&current).map_err(|e| {
        ApiError::from(io::Error::new(
            e.kind(),
            format!("{}: {}", path.display(), io_message(&e)),
        ))
    })?;
    Ok(len)
}

// ---------------------------------------------------------------- API

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteParams {
    paths: Vec<String>,
    passes: Option<u32>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PathResult {
    pub path: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub bytes: u64,
}

fn valid_passes(p: u32) -> bool {
    matches!(p, 1 | 3 | 7 | 35)
}

/// Securely delete one user-selected path (file, symlink or directory tree).
fn delete_one(
    ctx: &Ctx,
    safety: &Arc<Safety>,
    raw: &str,
    passes: u32,
    job: &Job,
) -> std::result::Result<u64, String> {
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err("path must be absolute".into());
    }
    let path = safety::normalize(&path);
    if safety.protected.is_protected(&path) {
        return Err("refused: path is protected".into());
    }
    if safety.protected.in_system_tree(&path) {
        return Err("refused: path is inside an operating system folder".into());
    }
    let parent = path.parent().ok_or("refused: path has no parent")?;
    let deleter = SafeDeleter::for_selection(safety.clone(), parent)
        .map_err(|e| e.message)?
        .with_secure(passes);
    let _ = ctx;
    let meta = fs::symlink_metadata(&path).map_err(|e| io_message(&e))?;
    if !meta.file_type().is_dir() {
        return deleter.remove_file(&path).map_err(|e| e.message());
    }
    // Directory tree: files first (securely), then directories bottom-up. Symlinks are
    // unlinked, never followed; other filesystems are not entered.
    let mut bytes = 0u64;
    let mut failures: Vec<String> = Vec::new();
    for entry in WalkDir::new(&path)
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

fn delete(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: DeleteParams = serde_json::from_value(params)?;
    if p.paths.is_empty() {
        return Err(ApiError::invalid_params("`paths` is empty"));
    }
    let s = settings::load(ctx);
    let passes = p.passes.unwrap_or(s.secure_delete.passes);
    if !valid_passes(passes) {
        return Err(ApiError::invalid_params(
            "`passes` must be one of 1, 3, 7 or 35",
        ));
    }
    let excludes = ExcludeSet::from_settings(&ctx.env, &s)?;
    let safety = Arc::new(Safety::new(&ctx.env, excludes));
    let total = p.paths.len();
    let mut results = Vec::with_capacity(total);
    let mut total_bytes = 0u64;
    for (i, raw) in p.paths.iter().enumerate() {
        job.progress(
            ProgressEvent::new("secure_delete")
                .fraction(i as f64 / total as f64)
                .counts(i as u64, total as u64)
                .message(raw.clone()),
        );
        if job.is_cancelled() {
            results.push(PathResult {
                path: raw.clone(),
                ok: false,
                error: Some("cancelled".into()),
                bytes: 0,
            });
            continue;
        }
        match delete_one(ctx, &safety, raw, passes, job) {
            Ok(bytes) => {
                total_bytes += bytes;
                results.push(PathResult {
                    path: raw.clone(),
                    ok: true,
                    error: None,
                    bytes,
                });
            }
            Err(msg) => results.push(PathResult {
                path: raw.clone(),
                ok: false,
                error: Some(msg),
                bytes: 0,
            }),
        }
    }
    Ok(serde_json::json!({ "results": results, "totalBytes": total_bytes }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dispatch;
    use crate::runner::MockRunner;
    use serde_json::json;

    fn ctx() -> (tempfile::TempDir, Ctx) {
        let d = tempfile::tempdir().unwrap();
        let c = Ctx::test(d.path(), MockRunner::new());
        fs::create_dir_all(&c.env.home).unwrap();
        (d, c)
    }

    #[test]
    fn schedule_lengths_and_shapes() {
        assert_eq!(overwrite_passes(1), vec![Pattern::Random]);
        assert_eq!(
            overwrite_passes(3),
            vec![Pattern::Byte(0), Pattern::Byte(0xFF), Pattern::Random]
        );
        let s7 = overwrite_passes(7);
        assert_eq!(s7.len(), 7);
        assert_eq!(&s7[..2], &[Pattern::Byte(0xFF), Pattern::Byte(0)]);
        assert!(s7[2..].iter().all(|p| *p == Pattern::Random));
        assert_eq!(overwrite_passes(35).len(), 35);
    }

    #[test]
    fn gutmann_matches_published_table() {
        let g = gutmann_patterns();
        assert_eq!(g.len(), 35);
        assert!(g[..4].iter().all(|p| *p == Pattern::Random));
        assert!(g[31..].iter().all(|p| *p == Pattern::Random));
        let seq = |v: [u8; 3]| Pattern::Bytes(v.to_vec());
        let expected_fixed: Vec<Pattern> = vec![
            seq([0x55, 0x55, 0x55]),
            seq([0xAA, 0xAA, 0xAA]),
            seq([0x92, 0x49, 0x24]),
            seq([0x49, 0x24, 0x92]),
            seq([0x24, 0x92, 0x49]),
            Pattern::Byte(0x00),
            Pattern::Byte(0x11),
            Pattern::Byte(0x22),
            Pattern::Byte(0x33),
            Pattern::Byte(0x44),
            Pattern::Byte(0x55),
            Pattern::Byte(0x66),
            Pattern::Byte(0x77),
            Pattern::Byte(0x88),
            Pattern::Byte(0x99),
            Pattern::Byte(0xAA),
            Pattern::Byte(0xBB),
            Pattern::Byte(0xCC),
            Pattern::Byte(0xDD),
            Pattern::Byte(0xEE),
            Pattern::Byte(0xFF),
            seq([0x92, 0x49, 0x24]),
            seq([0x49, 0x24, 0x92]),
            seq([0x24, 0x92, 0x49]),
            seq([0x6D, 0xB6, 0xDB]),
            seq([0xB6, 0xDB, 0x6D]),
            seq([0xDB, 0x6D, 0xB6]),
        ];
        assert_eq!(expected_fixed.len(), 27);
        assert_eq!(&g[4..31], expected_fixed.as_slice());
    }

    #[test]
    fn overwrite_in_place_reads_back_each_pattern() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.bin");
        // Not a multiple of 3 nor of the chunk size, and > 1 chunk, so the repeating
        // phase has to carry across chunk boundaries.
        let len = CHUNK + CHUNK / 2 + 7;
        fs::write(&p, vec![0x5Au8; len]).unwrap();

        for pat in [
            Pattern::Byte(0x00),
            Pattern::Byte(0xFF),
            Pattern::Bytes(vec![0x92, 0x49, 0x24]),
            Pattern::Bytes(vec![0x6D, 0xB6, 0xDB]),
        ] {
            assert_eq!(overwrite_file_in_place(&p, &pat).unwrap(), len as u64);
            let got = fs::read(&p).unwrap();
            assert_eq!(got.len(), len);
            let expect: Vec<u8> = match &pat {
                Pattern::Byte(b) => vec![*b; len],
                Pattern::Bytes(s) => (0..len).map(|i| s[i % s.len()]).collect(),
                Pattern::Random => unreachable!(),
            };
            assert!(got == expect, "pattern {pat:?} not written correctly");
        }

        // Random: content changes and is not constant.
        fs::write(&p, vec![0u8; 4096]).unwrap();
        overwrite_file_in_place(&p, &Pattern::Random).unwrap();
        let got = fs::read(&p).unwrap();
        assert_eq!(got.len(), 4096);
        assert!(got.iter().any(|b| *b != 0));
        let distinct: std::collections::HashSet<_> = got.iter().collect();
        assert!(distinct.len() > 100, "output does not look random");
    }

    #[test]
    fn overwrite_empty_file_is_fine() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("e");
        fs::write(&p, b"").unwrap();
        assert_eq!(overwrite_file_in_place(&p, &Pattern::Random).unwrap(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn overwrite_refuses_symlinks_and_non_files() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("t");
        fs::write(&target, b"important").unwrap();
        let link = d.path().join("l");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(overwrite_file_in_place(&link, &Pattern::Byte(0)).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"important");
        assert!(overwrite_file_in_place(d.path(), &Pattern::Byte(0)).is_err());
        assert!(secure_delete_file(&link, 1, &Job::detached()).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"important");
    }

    #[cfg(unix)]
    #[test]
    fn hard_linked_files_are_refused_and_left_intact() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        let b = d.path().join("b");
        fs::write(&a, vec![9u8; 100]).unwrap();
        fs::hard_link(&a, &b).unwrap();
        let e = secure_delete_file(&a, 1, &Job::detached()).unwrap_err();
        assert!(e.message.contains("hard links"), "{}", e.message);
        assert_eq!(fs::read(&a).unwrap(), vec![9u8; 100]);
        assert_eq!(fs::read(&b).unwrap(), vec![9u8; 100]);
    }

    #[test]
    fn secure_delete_file_removes_it_and_leaves_no_renamed_leftover() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secret-document.txt");
        fs::write(&p, vec![7u8; 5000]).unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(0usize));
        let e2 = events.clone();
        let job = Job::new(crate::job::CancelToken::new(), move |_| {
            *e2.lock().unwrap() += 1;
        });
        assert_eq!(secure_delete_file(&p, 3, &job).unwrap(), 5000);
        assert!(!p.exists());
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 0);
        assert!(*events.lock().unwrap() >= 3);
    }

    #[test]
    fn delete_method_handles_files_dirs_protected_and_errors() {
        let (_d, c) = ctx();
        let work = c.env.home.join("work");
        fs::create_dir_all(work.join("tree/sub")).unwrap();
        fs::write(work.join("a.txt"), "aaa").unwrap();
        fs::write(work.join("tree/b.txt"), "bb").unwrap();
        fs::write(work.join("tree/sub/c.txt"), "c").unwrap();
        let docs = c.env.home.join("Documents");
        fs::create_dir_all(&docs).unwrap();
        fs::write(docs.join("in-docs.txt"), "d").unwrap();

        let out = dispatch(
            &c,
            "secure_delete.delete",
            json!({"passes": 1, "paths": [
                work.join("a.txt"),
                work.join("tree"),
                work.join("missing"),
                "relative/path",
                c.env.home,
                docs,
                docs.join("in-docs.txt"),
            ]}),
            &Job::detached(),
        )
        .unwrap();
        let r = out["results"].as_array().unwrap();
        assert_eq!(r[0]["ok"], true);
        assert_eq!(r[0]["bytes"], 3);
        assert_eq!(r[1]["ok"], true);
        assert_eq!(r[1]["bytes"], 3);
        assert_eq!(r[2]["ok"], false);
        assert_eq!(r[3]["ok"], false);
        assert!(r[4]["error"].as_str().unwrap().contains("protected"));
        assert!(r[5]["error"].as_str().unwrap().contains("protected"));
        // A file inside a protected folder is a legitimate explicit selection.
        assert_eq!(r[6]["ok"], true);
        assert!(!work.join("a.txt").exists());
        assert!(!work.join("tree").exists());
        assert!(c.env.home.exists());
        assert!(c.env.home.join("Documents").exists());
        assert_eq!(out["totalBytes"], 3 + 3 + 1);
    }

    #[test]
    fn delete_method_refuses_operating_system_trees() {
        let (_d, c) = ctx();
        let f = c.env.sys_path("/usr/lib/thing.so");
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(&f, "x").unwrap();
        let out = dispatch(
            &c,
            "secure_delete.delete",
            json!({"paths": [f]}),
            &Job::detached(),
        )
        .unwrap();
        assert_eq!(out["results"][0]["ok"], false);
        assert!(out["results"][0]["error"]
            .as_str()
            .unwrap()
            .contains("operating system"));
        assert!(f.exists());
    }

    #[cfg(unix)]
    #[test]
    fn delete_method_never_follows_symlinks_in_trees() {
        let (_d, c) = ctx();
        let outside = c.env.home.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "keep").unwrap();
        let tree = c.env.home.join("tree");
        fs::create_dir_all(&tree).unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("dirlink")).unwrap();
        std::os::unix::fs::symlink(outside.join("keep.txt"), tree.join("filelink")).unwrap();
        fs::write(tree.join("own.txt"), "x").unwrap();
        let out = dispatch(
            &c,
            "secure_delete.delete",
            json!({"paths": [tree]}),
            &Job::detached(),
        )
        .unwrap();
        assert_eq!(out["results"][0]["ok"], true, "{out}");
        assert!(!tree.exists());
        assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"keep");

        // A symlink given directly is removed as a link.
        let link = c.env.home.join("toplink");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let out = dispatch(
            &c,
            "secure_delete.delete",
            json!({"paths": [link]}),
            &Job::detached(),
        )
        .unwrap();
        assert_eq!(out["results"][0]["ok"], true);
        assert!(outside.join("keep.txt").exists());
    }

    #[test]
    fn delete_method_validates_params_and_honors_excludes() {
        let (_d, c) = ctx();
        assert!(dispatch(
            &c,
            "secure_delete.delete",
            json!({"paths": []}),
            &Job::detached()
        )
        .is_err());
        let f = c.env.home.join("f");
        fs::write(&f, "x").unwrap();
        assert!(dispatch(
            &c,
            "secure_delete.delete",
            json!({"paths": [f], "passes": 2}),
            &Job::detached()
        )
        .is_err());
        settings::update(&c, |s| {
            s.exclude.push(settings::ExcludeEntry {
                id: "e1".into(),
                pattern: f.to_string_lossy().into_owned(),
            });
        })
        .unwrap();
        let out = dispatch(
            &c,
            "secure_delete.delete",
            json!({"paths": [f]}),
            &Job::detached(),
        )
        .unwrap();
        assert_eq!(out["results"][0]["ok"], false);
        assert!(f.exists());
    }
}
