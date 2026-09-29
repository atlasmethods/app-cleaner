//! The duplicate search: walk, group by cheap keys, drop hard links, then confirm by
//! content (partial hash, then full BLAKE3) in parallel.

use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use walkdir::WalkDir;

use crate::ctx::Ctx;
use crate::error::{ApiError, Result};
use crate::features::disk_analyzer::walk::{
    file_identity, has_system_attr, identity_from_meta, is_hidden, mtime_secs, FileId, SkipDirs,
    Throttle,
};
use crate::job::{Job, ProgressEvent};
use crate::safety::{io_message, ExcludeSet, Protected};

/// Bytes read from each end of a file for the partial hash.
pub const PARTIAL_BYTES: u64 = 4096;
const READ_BUF: usize = 256 * 1024;
const MAX_ERROR_SAMPLES: usize = 20;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct MatchBy {
    pub name: bool,
    pub size: bool,
    pub modified: bool,
    pub content: bool,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub match_by: MatchBy,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub include_hidden: bool,
    pub include_system: bool,
    pub skip_zero_byte: bool,
    pub follow_links: bool,
    /// Compare names ignoring case (Windows and macOS).
    pub case_insensitive_names: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DupFile {
    pub path: String,
    pub bytes: u64,
    /// Unix seconds.
    pub modified: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupKey {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<i64>,
    /// Hex BLAKE3 of the whole content (content matches only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub group_id: String,
    pub key: GroupKey,
    pub files: Vec<DupFile>,
    pub wasted_bytes: u64,
}

#[derive(Debug, Default)]
pub struct Found {
    pub groups: Vec<Group>,
    pub scanned_files: u64,
    pub error_count: u64,
    pub error_samples: Vec<String>,
}

struct Candidate {
    path: String,
    bytes: u64,
    modified: i64,
    name_key: String,
    id: Option<FileId>,
}

/// Bytes wasted by a group: everything except its biggest member.
pub fn wasted_of(files: &[DupFile]) -> u64 {
    let total: u64 = files.iter().map(|f| f.bytes).sum();
    total - files.iter().map(|f| f.bytes).max().unwrap_or(0)
}

// ---------------------------------------------------------------- hashing

#[derive(Debug)]
pub enum HashError {
    Cancelled,
    Io(io::Error),
}

impl From<io::Error> for HashError {
    fn from(e: io::Error) -> Self {
        HashError::Io(e)
    }
}

/// Streaming BLAKE3 of the whole file. `on_bytes` sees every chunk size; cancellation is
/// polled between chunks.
pub fn hash_file(
    path: &Path,
    job: &Job,
    mut on_bytes: impl FnMut(u64),
) -> std::result::Result<blake3::Hash, HashError> {
    let mut f = File::open(path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; READ_BUF];
    loop {
        if job.is_cancelled() {
            return Err(HashError::Cancelled);
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        on_bytes(n as u64);
    }
    Ok(h.finalize())
}

/// Hash of the first and last [`PARTIAL_BYTES`] of a file of `size` bytes. A file that
/// fits in both windows (`size <= 2 * PARTIAL_BYTES`) is read completely, so its partial
/// hash *is* its full hash (the second element is `true`).
pub fn partial_hash(path: &Path, size: u64) -> io::Result<(blake3::Hash, bool)> {
    let mut f = File::open(path)?;
    let mut h = blake3::Hasher::new();
    if size <= 2 * PARTIAL_BYTES {
        let mut buf = Vec::with_capacity(size as usize);
        f.read_to_end(&mut buf)?;
        h.update(&buf);
        return Ok((h.finalize(), true));
    }
    let mut head = vec![0u8; PARTIAL_BYTES as usize];
    f.read_exact(&mut head)?;
    h.update(&head);
    f.seek(SeekFrom::Start(size - PARTIAL_BYTES))?;
    let mut tail = vec![0u8; PARTIAL_BYTES as usize];
    f.read_exact(&mut tail)?;
    h.update(&tail);
    Ok((h.finalize(), false))
}

// ---------------------------------------------------------------- walking

struct Errors {
    count: u64,
    samples: Vec<String>,
}

impl Errors {
    fn add(&mut self, path: &Path, msg: impl std::fmt::Display) {
        self.count += 1;
        if self.samples.len() < MAX_ERROR_SAMPLES {
            self.samples.push(format!("{}: {msg}", path.display()));
        }
    }
}

fn collect(
    ctx: &Ctx,
    roots: &[PathBuf],
    excludes: &ExcludeSet,
    opts: &Options,
    job: &Job,
    errors: &mut Errors,
) -> Result<Vec<Candidate>> {
    let protected = Protected::new(&ctx.env);
    let skip = SkipDirs::new(&ctx.env);
    let mut out: Vec<Candidate> = Vec::new();
    let throttle = Throttle::new(100);
    let want_name = opts.match_by.name;
    for root in roots {
        let walker = WalkDir::new(root)
            .follow_links(opts.follow_links)
            .same_file_system(true)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                let p = e.path();
                if e.file_type().is_dir() {
                    if skip.contains(p) {
                        return false;
                    }
                    if !opts.include_system && protected.in_system_tree(p) {
                        return false;
                    }
                }
                if excludes.is_excluded(p) {
                    return false;
                }
                if !opts.include_hidden || !opts.include_system {
                    if let Ok(m) = e.metadata() {
                        if !opts.include_hidden && is_hidden(e.file_name(), &m) {
                            return false;
                        }
                        if !opts.include_system && has_system_attr(&m) {
                            return false;
                        }
                    }
                }
                true
            });
        for ent in walker {
            if out.len().is_multiple_of(256) {
                job.check_cancelled()?;
            }
            let ent = match ent {
                Ok(e) => e,
                Err(e) => {
                    let p = e.path().map(Path::to_path_buf).unwrap_or_default();
                    errors.add(&p, e);
                    continue;
                }
            };
            // Symlinked files are never candidates (their target is found on its own).
            if !ent.file_type().is_file() || ent.path_is_symlink() {
                continue;
            }
            let meta = match ent.metadata() {
                Ok(m) => m,
                Err(e) => {
                    errors.add(ent.path(), e);
                    continue;
                }
            };
            let bytes = meta.len();
            if (opts.skip_zero_byte && bytes == 0)
                || opts.min_size.is_some_and(|m| bytes < m)
                || opts.max_size.is_some_and(|m| bytes > m)
            {
                continue;
            }
            let Some(path) = ent.path().to_str() else {
                errors.add(ent.path(), "the name is not valid UTF-8; skipped");
                continue;
            };
            let name_key = if want_name {
                let n = ent.file_name().to_string_lossy();
                if opts.case_insensitive_names {
                    n.to_lowercase()
                } else {
                    n.into_owned()
                }
            } else {
                String::new()
            };
            out.push(Candidate {
                path: path.to_string(),
                bytes,
                modified: mtime_secs(&meta),
                name_key,
                id: identity_from_meta(&meta).map(|i| i.id),
            });
            if throttle.ready() {
                job.progress(
                    ProgressEvent::new("scan").message(format!("{} files found", out.len())),
                );
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- grouping

#[derive(Hash, PartialEq, Eq)]
struct Key(Option<String>, Option<u64>, Option<i64>);

fn key_of(c: &Candidate, m: &MatchBy) -> Key {
    Key(
        m.name.then(|| c.name_key.clone()),
        // Equal content implies equal size, so size always narrows a content search.
        (m.size || m.content).then_some(c.bytes),
        m.modified.then_some(c.modified),
    )
}

/// Keep one name per file: later names of the same `(dev, inode)` are dropped.
fn dedupe_links(group: Vec<Candidate>) -> Vec<Candidate> {
    let mut seen: std::collections::HashSet<FileId> = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(group.len());
    for mut c in group {
        if c.id.is_none() {
            // Windows: identity needs a handle; only worth it for group members.
            if let Ok(meta) = fs::symlink_metadata(&c.path) {
                c.id = file_identity(Path::new(&c.path), &meta).map(|i| i.id);
            }
        }
        match c.id {
            Some(id) if !seen.insert(id) => {}
            _ => out.push(c),
        }
    }
    out
}

fn to_file(c: &Candidate) -> DupFile {
    DupFile {
        path: c.path.clone(),
        bytes: c.bytes,
        modified: c.modified,
    }
}

fn order_files(files: &mut [DupFile]) {
    files.sort_by(|a, b| {
        a.modified
            .cmp(&b.modified)
            .then_with(|| a.path.cmp(&b.path))
    });
}

fn make_group(files: Vec<DupFile>, key: GroupKey) -> Group {
    let wasted = wasted_of(&files);
    Group {
        group_id: String::new(),
        key,
        files,
        wasted_bytes: wasted,
    }
}

/// Search `roots` for duplicates.
pub fn find(
    ctx: &Ctx,
    roots: &[PathBuf],
    excludes: &ExcludeSet,
    opts: &Options,
    job: &Job,
) -> Result<Found> {
    let mut errors = Errors {
        count: 0,
        samples: Vec::new(),
    };
    let candidates = collect(ctx, roots, excludes, opts, job, &mut errors)?;
    let scanned_files = candidates.len() as u64;
    job.check_cancelled()?;

    let mut by_key: HashMap<Key, Vec<Candidate>> = HashMap::new();
    for c in candidates {
        by_key
            .entry(key_of(&c, &opts.match_by))
            .or_default()
            .push(c);
    }
    let mut stage1: Vec<(Key, Vec<Candidate>)> = by_key
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(k, v)| (k, dedupe_links(v)))
        .filter(|(_, v)| v.len() > 1)
        .collect();
    // Deterministic order regardless of hash-map iteration.
    stage1.sort_by(|a, b| a.1[0].path.cmp(&b.1[0].path));
    job.check_cancelled()?;

    let mut groups: Vec<Group> = Vec::new();
    if !opts.match_by.content {
        for (k, members) in stage1 {
            let mut files: Vec<DupFile> = members.iter().map(to_file).collect();
            order_files(&mut files);
            let key = GroupKey {
                name: k.0.as_ref().map(|_| {
                    Path::new(&files[0].path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                }),
                bytes: k.1.filter(|_| opts.match_by.size),
                modified: k.2,
                hash: None,
            };
            groups.push(make_group(files, key));
        }
    } else {
        let err_lock = Mutex::new(&mut errors);
        groups = confirm_by_content(stage1, opts, job, &err_lock)?;
    }
    groups.sort_by(|a, b| {
        b.wasted_bytes
            .cmp(&a.wasted_bytes)
            .then_with(|| a.files[0].path.cmp(&b.files[0].path))
    });
    for (i, g) in groups.iter_mut().enumerate() {
        g.group_id = format!("g{i}");
    }
    Ok(Found {
        groups,
        scanned_files,
        error_count: errors.count,
        error_samples: errors.samples,
    })
}

fn confirm_by_content(
    stage1: Vec<(Key, Vec<Candidate>)>,
    opts: &Options,
    job: &Job,
    errors: &Mutex<&mut Errors>,
) -> Result<Vec<Group>> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(hash_threads())
        .build()
        .map_err(|e| ApiError::internal(format!("cannot start hashing threads: {e}")))?;
    let throttle = Throttle::new(100);

    // ---- stage 2: partial hash of every member
    let flat: Vec<(usize, Candidate)> = stage1
        .into_iter()
        .enumerate()
        .flat_map(|(gi, (_, v))| v.into_iter().map(move |c| (gi, c)))
        .collect();
    let total_files = flat.len() as u64;
    let done_files = AtomicU64::new(0);
    let partial: Vec<Option<(blake3::Hash, bool)>> = pool.install(|| {
        flat.par_iter()
            .map(|(_, c)| {
                if job.is_cancelled() {
                    return None;
                }
                let r = match partial_hash(Path::new(&c.path), c.bytes) {
                    Ok(r) => Some(r),
                    Err(e) => {
                        errors
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .add(Path::new(&c.path), io_message(&e));
                        None
                    }
                };
                let d = done_files.fetch_add(1, Ordering::Relaxed) + 1;
                if throttle.ready() {
                    job.progress(
                        ProgressEvent::new("hash")
                            .fraction(d as f64 / total_files.max(1) as f64)
                            .counts(d, total_files)
                            .message("Comparing file starts and ends"),
                    );
                }
                r
            })
            .collect()
    });
    job.check_cancelled()?;

    // Regroup by (group, partial hash); keep only real candidates for a full hash.
    type Sub = Vec<(Candidate, bool)>;
    let mut subs: HashMap<(usize, [u8; 32]), Sub> = HashMap::new();
    let mut order: Vec<(usize, [u8; 32])> = Vec::new();
    for ((gi, c), p) in flat.into_iter().zip(partial) {
        let Some((h, full)) = p else { continue };
        let k = (gi, *h.as_bytes());
        let e = subs.entry(k).or_default();
        if e.is_empty() {
            order.push(k);
        }
        e.push((c, full));
    }
    let mut work: Vec<(usize, [u8; 32], Sub)> = order
        .into_iter()
        .filter_map(|k| subs.remove(&k).map(|v| (k.0, k.1, v)))
        .filter(|(_, _, v)| v.len() > 1)
        .collect();
    work.sort_by(|a, b| a.2[0].0.path.cmp(&b.2[0].0.path));

    // ---- stage 3: full hash where the partial hash did not already cover the file
    let to_hash_bytes: u64 = work
        .iter()
        .filter(|(_, _, v)| !v[0].1)
        .flat_map(|(_, _, v)| v.iter().map(|(c, _)| c.bytes))
        .sum();
    let hashed = AtomicU64::new(0);
    let mut groups: Vec<Group> = Vec::new();
    for (_, phash, members) in work {
        job.check_cancelled()?;
        let is_full = members[0].1;
        let hashes: Vec<Option<[u8; 32]>> = if is_full {
            vec![Some(phash); members.len()]
        } else {
            pool.install(|| {
                members
                    .par_iter()
                    .map(|(c, _)| {
                        match hash_file(Path::new(&c.path), job, |n| {
                            let d = hashed.fetch_add(n, Ordering::Relaxed) + n;
                            if throttle.ready() {
                                job.progress(
                                    ProgressEvent::new("hash")
                                        .fraction(d as f64 / to_hash_bytes.max(1) as f64)
                                        .counts(d, to_hash_bytes)
                                        .message("Reading files"),
                                );
                            }
                        }) {
                            Ok(h) => Some(*h.as_bytes()),
                            Err(HashError::Cancelled) => None,
                            Err(HashError::Io(e)) => {
                                errors
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .add(Path::new(&c.path), io_message(&e));
                                None
                            }
                        }
                    })
                    .collect()
            })
        };
        job.check_cancelled()?;
        let mut by_hash: HashMap<[u8; 32], Vec<&Candidate>> = HashMap::new();
        for ((c, _), h) in members.iter().zip(hashes) {
            if let Some(h) = h {
                by_hash.entry(h).or_default().push(c);
            }
        }
        let mut sets: Vec<([u8; 32], Vec<&Candidate>)> =
            by_hash.into_iter().filter(|(_, v)| v.len() > 1).collect();
        sets.sort_by(|a, b| a.1[0].path.cmp(&b.1[0].path));
        for (h, cs) in sets {
            let mut files: Vec<DupFile> = cs.iter().map(|c| to_file(c)).collect();
            order_files(&mut files);
            let key = GroupKey {
                name: opts.match_by.name.then(|| {
                    Path::new(&files[0].path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                }),
                bytes: Some(files[0].bytes),
                modified: opts.match_by.modified.then_some(files[0].modified),
                hash: Some(blake3::Hash::from_bytes(h).to_hex().to_string()),
            };
            groups.push(make_group(files, key));
        }
    }
    Ok(groups)
}

fn hash_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8)
}
