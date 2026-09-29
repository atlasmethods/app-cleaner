//! Parallel directory walk: per-category totals, a folder tree with subtree sizes, and
//! the biggest files (bounded) for paging.
//!
//! Rules of the walk:
//! * symbolic links (and Windows junctions / reparse points) are never followed and
//!   never counted;
//! * a walk stays on the filesystem of the root it started from ([`same_filesystem`]);
//! * `/proc`, `/sys`, `/dev` (and macOS equivalents) are skipped;
//! * sizes are apparent sizes (`st_size`), not allocated blocks: a sparse file counts at
//!   its full length;
//! * a file with several hard links (Unix) is counted once, under the first name met.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Mutex;
use std::time::Instant;

use crate::ctx::Ctx;
use crate::error::{ApiError, Result};
use crate::job::{Job, ProgressEvent};
use crate::safety::io_message;

use super::category::{classify, Category};
use super::walk::{dev_of, identity_from_meta, mtime_secs, same_filesystem, FileId, SkipDirs};

/// Largest number of files kept per scan (the biggest ones). Totals always cover all.
pub const MAX_STORED_FILES: usize = 200_000;
/// Largest number of folders with their own tree node. Deeper folders are folded into
/// their nearest recorded ancestor.
pub const MAX_DIR_NODES: usize = 1_000_000;
pub const MAX_ERROR_SAMPLES: usize = 20;
pub const NO_PARENT: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub files: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: Box<str>,
    pub bytes: u64,
    /// Unix seconds.
    pub modified: i64,
    pub category: Category,
    /// Index into [`ScanData::roots`].
    pub root: u16,
}

impl Ord for FileEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.bytes
            .cmp(&other.bytes)
            .then_with(|| other.path.cmp(&self.path))
    }
}
impl PartialOrd for FileEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone)]
pub struct DirNode {
    pub name: Box<str>,
    pub parent: u32,
    pub children: Vec<u32>,
    pub own_files: u64,
    pub own_bytes: u64,
    pub sub_files: u64,
    pub sub_bytes: u64,
}

/// Everything a finished scan keeps in memory.
pub struct ScanData {
    pub id: String,
    pub roots: Vec<PathBuf>,
    pub root_nodes: Vec<u32>,
    pub nodes: Vec<DirNode>,
    /// Biggest files first (bytes desc, then path).
    pub files: Vec<FileEntry>,
    pub totals: [Totals; 7],
    pub truncated: bool,
    pub error_count: u64,
    pub error_samples: Vec<String>,
    pub duration_ms: u64,
}

impl ScanData {
    pub fn total_files(&self) -> u64 {
        self.totals.iter().map(|t| t.files).sum()
    }
    pub fn total_bytes(&self) -> u64 {
        self.totals.iter().map(|t| t.bytes).sum()
    }

    /// The deepest recorded folder node on the way to `dir` (which must lie below one of
    /// the roots), its root index, and whether every component was found.
    pub fn find_node(&self, dir: &Path) -> Option<(usize, u32, bool)> {
        for (ri, root) in self.roots.iter().enumerate() {
            let Ok(rel) = dir.strip_prefix(root) else {
                continue;
            };
            let mut node = self.root_nodes[ri];
            let mut exact = true;
            for comp in rel.components() {
                let name = comp.as_os_str().to_string_lossy();
                let next = self.nodes[node as usize]
                    .children
                    .iter()
                    .copied()
                    .find(|&c| *self.nodes[c as usize].name == *name);
                match next {
                    Some(c) => node = c,
                    None => {
                        exact = false;
                        break;
                    }
                }
            }
            return Some((ri, node, exact));
        }
        None
    }

    /// Path of a node (root path plus the names down to it).
    pub fn node_path(&self, node: u32) -> PathBuf {
        let mut names = Vec::new();
        let mut cur = node;
        while self.nodes[cur as usize].parent != NO_PARENT {
            names.push(self.nodes[cur as usize].name.clone());
            cur = self.nodes[cur as usize].parent;
        }
        let root_idx = self
            .root_nodes
            .iter()
            .position(|&r| r == cur)
            .unwrap_or_default();
        let mut p = self.roots[root_idx].clone();
        for n in names.iter().rev() {
            p.push(&**n);
        }
        p
    }

    /// Forget files that were deleted: fix category totals, folder sizes and the list.
    pub fn apply_removed(&mut self, removed: &HashSet<Box<str>>) {
        if removed.is_empty() {
            return;
        }
        let mut gone: Vec<FileEntry> = Vec::new();
        self.files.retain(|e| {
            if removed.contains(&e.path) {
                gone.push(e.clone());
                false
            } else {
                true
            }
        });
        for e in gone {
            let t = &mut self.totals[e.category.index()];
            t.files = t.files.saturating_sub(1);
            t.bytes = t.bytes.saturating_sub(e.bytes);
            let dir = Path::new(&*e.path).parent().map(Path::to_path_buf);
            if let Some((_, mut node, _)) = dir.and_then(|d| self.find_node(&d)) {
                let n = &mut self.nodes[node as usize];
                n.own_files = n.own_files.saturating_sub(1);
                n.own_bytes = n.own_bytes.saturating_sub(e.bytes);
                loop {
                    let n = &mut self.nodes[node as usize];
                    n.sub_files = n.sub_files.saturating_sub(1);
                    n.sub_bytes = n.sub_bytes.saturating_sub(e.bytes);
                    if n.parent == NO_PARENT {
                        break;
                    }
                    node = n.parent;
                }
            }
        }
    }
}

pub struct ScanOptions {
    /// Which categories to count (indexed by [`Category::index`]).
    pub cats: [bool; 7],
    pub max_files: usize,
}

impl ScanOptions {
    pub fn all() -> Self {
        Self {
            cats: [true; 7],
            max_files: MAX_STORED_FILES,
        }
    }
}

struct State {
    nodes: Vec<DirNode>,
    /// Min-heap on size: the smallest kept file is on top.
    heap: BinaryHeap<Reverse<FileEntry>>,
    totals: [Totals; 7],
    truncated: bool,
    error_count: u64,
    error_samples: Vec<String>,
}

struct Shared<'a> {
    job: &'a Job,
    opts: &'a ScanOptions,
    skip: SkipDirs,
    state: Mutex<State>,
    seen: Mutex<HashSet<FileId>>,
    files_seen: AtomicU64,
    bytes_seen: AtomicU64,
    last_emit_ms: AtomicU64,
    start: Instant,
}

impl Shared<'_> {
    fn error(&self, path: &Path, e: &io::Error) {
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        st.error_count += 1;
        if st.error_samples.len() < MAX_ERROR_SAMPLES {
            st.error_samples
                .push(format!("{}: {}", path.display(), io_message(e)));
        }
    }

    /// At most one progress event per 100 ms across all threads.
    fn maybe_emit(&self) {
        let now = self.start.elapsed().as_millis() as u64;
        let last = self.last_emit_ms.load(AtomicOrdering::Relaxed);
        if now.saturating_sub(last) < 100 {
            return;
        }
        if self
            .last_emit_ms
            .compare_exchange(last, now, AtomicOrdering::Relaxed, AtomicOrdering::Relaxed)
            .is_err()
        {
            return;
        }
        let files = self.files_seen.load(AtomicOrdering::Relaxed);
        let bytes = self.bytes_seen.load(AtomicOrdering::Relaxed);
        let mut ev = ProgressEvent::new("scan").message(format!("{files} files, {bytes} bytes"));
        ev.current = Some(files);
        self.job.progress(ev);
    }
}

struct Task {
    dir: PathBuf,
    node: u32,
    root: u16,
    root_dev: Option<u64>,
}

fn walk_dir<'s>(scope: &rayon::Scope<'s>, sh: &'s Shared<'s>, task: Task) {
    if sh.job.is_cancelled() {
        return;
    }
    let rd = match fs::read_dir(&task.dir) {
        Ok(r) => r,
        Err(e) => {
            sh.error(&task.dir, &e);
            return;
        }
    };
    let mut local: Vec<FileEntry> = Vec::new();
    let mut local_totals = [Totals::default(); 7];
    let mut own = Totals::default();
    let mut subdirs: Vec<(PathBuf, String)> = Vec::new();
    let mut n_entries = 0u32;
    for ent in rd {
        n_entries += 1;
        if n_entries.is_multiple_of(512) && sh.job.is_cancelled() {
            return;
        }
        let ent = match ent {
            Ok(e) => e,
            Err(e) => {
                sh.error(&task.dir, &e);
                continue;
            }
        };
        let ft = match ent.file_type() {
            Ok(t) => t,
            Err(e) => {
                sh.error(&ent.path(), &e);
                continue;
            }
        };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            let p = ent.path();
            if sh.skip.contains(&p) {
                continue;
            }
            match ent.metadata() {
                Ok(m) => {
                    if !same_filesystem(task.root_dev, dev_of(&m)) {
                        continue;
                    }
                }
                Err(e) => {
                    sh.error(&p, &e);
                    continue;
                }
            }
            subdirs.push((p, ent.file_name().to_string_lossy().into_owned()));
        } else if ft.is_file() {
            let name = ent.file_name();
            let cat = classify(&name);
            if !sh.opts.cats[cat.index()] {
                continue;
            }
            let meta = match ent.metadata() {
                Ok(m) => m,
                Err(e) => {
                    sh.error(&ent.path(), &e);
                    continue;
                }
            };
            if let Some(idn) = identity_from_meta(&meta) {
                if idn.nlink > 1 {
                    let mut seen = sh.seen.lock().unwrap_or_else(|p| p.into_inner());
                    if !seen.insert(idn.id) {
                        continue; // another name of a file already counted
                    }
                }
            }
            let bytes = meta.len();
            let t = &mut local_totals[cat.index()];
            t.files += 1;
            t.bytes += bytes;
            own.files += 1;
            own.bytes += bytes;
            let path = ent.path();
            if let Some(s) = path.to_str() {
                local.push(FileEntry {
                    path: s.into(),
                    bytes,
                    modified: mtime_secs(&meta),
                    category: cat,
                    root: task.root,
                });
            }
        }
    }

    let n_files = own.files;
    sh.files_seen.fetch_add(n_files, AtomicOrdering::Relaxed);
    sh.bytes_seen.fetch_add(own.bytes, AtomicOrdering::Relaxed);

    let child_nodes: Vec<u32> = {
        let mut guard = sh.state.lock().unwrap_or_else(|p| p.into_inner());
        let st: &mut State = &mut guard;
        for (i, t) in local_totals.iter().enumerate() {
            st.totals[i].files += t.files;
            st.totals[i].bytes += t.bytes;
        }
        {
            let n = &mut st.nodes[task.node as usize];
            n.own_files += own.files;
            n.own_bytes += own.bytes;
        }
        for e in local {
            if st.heap.len() < sh.opts.max_files {
                st.heap.push(Reverse(e));
            } else if let Some(Reverse(min)) = st.heap.peek() {
                st.truncated = true;
                if e > *min {
                    st.heap.pop();
                    st.heap.push(Reverse(e));
                }
            }
        }
        subdirs
            .iter()
            .map(|(_, name)| {
                if st.nodes.len() < MAX_DIR_NODES {
                    let id = st.nodes.len() as u32;
                    st.nodes.push(DirNode {
                        name: name.as_str().into(),
                        parent: task.node,
                        children: Vec::new(),
                        own_files: 0,
                        own_bytes: 0,
                        sub_files: 0,
                        sub_bytes: 0,
                    });
                    st.nodes[task.node as usize].children.push(id);
                    id
                } else {
                    task.node // folded into the nearest recorded ancestor
                }
            })
            .collect()
    };
    sh.maybe_emit();
    for ((dir, _), node) in subdirs.into_iter().zip(child_nodes) {
        let t = Task {
            dir,
            node,
            root: task.root,
            root_dev: task.root_dev,
        };
        scope.spawn(move |s| walk_dir(s, sh, t));
    }
}

/// Walk `roots` (absolute, existing directories, none inside another).
pub fn run_scan(
    ctx: &Ctx,
    id: String,
    roots: Vec<PathBuf>,
    opts: &ScanOptions,
    job: &Job,
) -> Result<ScanData> {
    let start = Instant::now();
    let mut nodes: Vec<DirNode> = Vec::new();
    let mut root_nodes = Vec::new();
    let mut root_devs = Vec::new();
    for r in &roots {
        let meta = fs::symlink_metadata(r).map_err(|e| {
            ApiError::from(io::Error::new(
                e.kind(),
                format!("{}: {}", r.display(), io_message(&e)),
            ))
        })?;
        root_devs.push(dev_of(&meta));
        root_nodes.push(nodes.len() as u32);
        nodes.push(DirNode {
            name: r.to_string_lossy().into_owned().into(),
            parent: NO_PARENT,
            children: Vec::new(),
            own_files: 0,
            own_bytes: 0,
            sub_files: 0,
            sub_bytes: 0,
        });
    }
    let sh = Shared {
        job,
        opts,
        skip: SkipDirs::new(&ctx.env),
        state: Mutex::new(State {
            nodes,
            heap: BinaryHeap::new(),
            totals: [Totals::default(); 7],
            truncated: false,
            error_count: 0,
            error_samples: Vec::new(),
        }),
        seen: Mutex::new(HashSet::new()),
        files_seen: AtomicU64::new(0),
        bytes_seen: AtomicU64::new(0),
        last_emit_ms: AtomicU64::new(0),
        start,
    };
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(|e| ApiError::internal(format!("cannot start scan threads: {e}")))?;
    pool.scope(|s| {
        for (i, root) in roots.iter().enumerate() {
            let t = Task {
                dir: root.clone(),
                node: root_nodes[i],
                root: i as u16,
                root_dev: root_devs[i],
            };
            let shr = &sh;
            s.spawn(move |s| walk_dir(s, shr, t));
        }
    });
    job.check_cancelled()?;

    let st = sh.state.into_inner().unwrap_or_else(|p| p.into_inner());
    let mut nodes = st.nodes;
    for n in nodes.iter_mut() {
        n.sub_files = n.own_files;
        n.sub_bytes = n.own_bytes;
    }
    // Children always have a larger index than their parent, so one reverse pass
    // completes every subtree.
    for i in (0..nodes.len()).rev() {
        let p = nodes[i].parent;
        if p != NO_PARENT {
            let (f, b) = (nodes[i].sub_files, nodes[i].sub_bytes);
            nodes[p as usize].sub_files += f;
            nodes[p as usize].sub_bytes += b;
        }
    }
    let mut files: Vec<FileEntry> = st.heap.into_iter().map(|Reverse(e)| e).collect();
    files.sort_by(|a, b| b.cmp(a));
    Ok(ScanData {
        id,
        roots,
        root_nodes,
        nodes,
        files,
        totals: st.totals,
        truncated: st.truncated,
        error_count: st.error_count,
        error_samples: st.error_samples,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
