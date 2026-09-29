//! Disk analyzer: break down disk usage by file type, drill into folders and delete
//! files found that way.
//!
//! Methods (all JSON, camelCase):
//! - `disk_analyzer.list_drives` -> `[{name, mount, fs, total, available, removable}]`
//! - `disk_analyzer.scan { paths, categories? }` -> `{scanId, roots, totals, totalFiles,
//!   totalBytes, stored, truncated, errors: {count, samples}, durationMs}`
//! - `disk_analyzer.files { scanId, category?, folder?, sort, offset, limit }` -> paged list
//! - `disk_analyzer.tree { scanId, path? }` -> one folder level with subtree sizes
//! - `disk_analyzer.delete { scanId, paths }` -> per-path results (only files that are in
//!   that scan's results, only when unchanged since, through `SafeDeleter`)
//! - `disk_analyzer.open_folder { path }` -> reveal in the file manager
//!
//! Scans stay in memory (the last three, least recently used evicted) so the client can
//! page through them. Each keeps totals for *every* file but only the
//! [`scan::MAX_STORED_FILES`] biggest files individually.

pub mod cache;
pub mod category;
pub mod drives;
pub mod scan;
pub mod walk;

#[cfg(test)]
mod tests;

use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::api::Registry;
use crate::ctx::{Ctx, Os};
use crate::error::{ApiError, Result};
use crate::features::settings;
use crate::fsutil::random_id;
use crate::job::{Job, ProgressEvent};
use crate::safety::{self, io_message, is_within, ExcludeSet, SafeDeleter, Safety};

use cache::ScanStore;
use category::Category;
use scan::{FileEntry, ScanData, ScanOptions, NO_PARENT};

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "disk_analyzer.list_drives",
    "disk_analyzer.scan",
    "disk_analyzer.files",
    "disk_analyzer.tree",
    "disk_analyzer.delete",
    "disk_analyzer.open_folder",
];

pub fn register(r: &mut Registry) {
    r.add("disk_analyzer.list_drives", list_drives);
    r.add("disk_analyzer.scan", scan_handler);
    r.add("disk_analyzer.files", files);
    r.add("disk_analyzer.tree", tree);
    r.add("disk_analyzer.delete", delete);
    r.add("disk_analyzer.open_folder", open_folder);
}

/// Most scans kept in memory at once.
pub const MAX_SCANS: usize = 3;
static SCANS: ScanStore<ScanData> = ScanStore::new(MAX_SCANS);

fn get_scan(ctx: &Ctx, id: &str) -> Result<Arc<Mutex<ScanData>>> {
    SCANS
        .get(&ctx.env.data_dir, id)
        .ok_or_else(|| ApiError::not_found("unknown or expired scan; run the analysis again"))
}

fn lock(s: &Mutex<ScanData>) -> std::sync::MutexGuard<'_, ScanData> {
    s.lock().unwrap_or_else(|p| p.into_inner())
}

// ---------------------------------------------------------------- list_drives

fn list_drives(ctx: &Ctx, _params: Value, _job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(drives::list_volumes(ctx))?)
}

// ---------------------------------------------------------------- scan

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanParams {
    paths: Vec<String>,
    categories: Option<Vec<Category>>,
}

fn totals_json(data: &ScanData) -> Value {
    let mut m = Map::new();
    for c in Category::ALL {
        let t = data.totals[c.index()];
        m.insert(
            c.key().to_string(),
            json!({"files": t.files, "bytes": t.bytes}),
        );
    }
    Value::Object(m)
}

fn errors_json(data: &ScanData) -> Value {
    json!({"count": data.error_count, "samples": data.error_samples})
}

fn summary_json(data: &ScanData) -> Value {
    json!({
        "scanId": data.id,
        "roots": data.roots.iter().map(|r| r.to_string_lossy()).collect::<Vec<_>>(),
        "totals": totals_json(data),
        "totalFiles": data.total_files(),
        "totalBytes": data.total_bytes(),
        "stored": data.files.len(),
        "truncated": data.truncated,
        "errors": errors_json(data),
        "durationMs": data.duration_ms,
    })
}

/// Validate and canonicalize the scan roots; drops roots nested inside another root.
fn resolve_roots(ctx: &Ctx, raw: &[String]) -> Result<Vec<PathBuf>> {
    if raw.is_empty() {
        return Err(ApiError::invalid_params("`paths` is empty"));
    }
    let skip = walk::SkipDirs::new(&ctx.env);
    let mut roots: Vec<PathBuf> = Vec::new();
    for r in raw {
        let p = walk::require_absolute(r).map_err(ApiError::invalid_params)?;
        let canon = safety::canonicalize(&p).map_err(|e| {
            ApiError::from(std::io::Error::new(
                e.kind(),
                format!("{r}: {}", io_message(&e)),
            ))
        })?;
        if !canon.is_dir() {
            return Err(ApiError::invalid_params(format!("not a folder: {r}")));
        }
        if skip.contains(&canon) {
            return Err(ApiError::invalid_params(format!(
                "{r} is a system pseudo-filesystem and cannot be analyzed"
            )));
        }
        roots.push(canon);
    }
    // Shortest first, then drop everything inside an earlier root (and exact repeats).
    roots.sort_by_key(|r| r.components().count());
    let mut out: Vec<PathBuf> = Vec::new();
    for r in roots {
        if !out.iter().any(|o| is_within(&r, o)) {
            out.push(r);
        }
    }
    Ok(out)
}

fn scan_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: ScanParams = serde_json::from_value(params)?;
    let roots = resolve_roots(ctx, &p.paths)?;
    let mut opts = ScanOptions::all();
    if let Some(cats) = &p.categories {
        if !cats.is_empty() {
            opts.cats = [false; 7];
            for c in cats {
                opts.cats[c.index()] = true;
            }
        }
    }
    let id = format!("da-{}", random_id());
    let data = scan::run_scan(ctx, id.clone(), roots, &opts, job)?;
    let summary = summary_json(&data);
    SCANS.insert(&ctx.env.data_dir, id, data);
    Ok(summary)
}

// ---------------------------------------------------------------- files

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesParams {
    scan_id: String,
    category: Option<Category>,
    folder: Option<String>,
    #[serde(default)]
    sort: SortKey,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum SortKey {
    /// Biggest first.
    #[default]
    Size,
    /// A to Z, case-insensitive.
    Name,
    /// Newest first.
    Modified,
}

fn file_name(e: &FileEntry) -> &str {
    Path::new(&*e.path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&e.path)
}

fn entry_json(e: &FileEntry) -> Value {
    json!({
        "path": &*e.path,
        "name": file_name(e),
        "bytes": e.bytes,
        "modified": e.modified,
        "category": e.category.key(),
    })
}

fn files(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: FilesParams = serde_json::from_value(params)?;
    let limit = p.limit.unwrap_or(100).clamp(1, 1000);
    let scan = get_scan(ctx, &p.scan_id)?;
    let data = lock(&scan);
    let folder = p
        .folder
        .as_deref()
        .map(|f| walk::require_absolute(f).map_err(ApiError::invalid_params))
        .transpose()?;
    let mut sel: Vec<&FileEntry> = data
        .files
        .iter()
        .filter(|e| p.category.is_none_or(|c| e.category == c))
        .filter(|e| {
            folder
                .as_deref()
                .is_none_or(|f| is_within(Path::new(&*e.path), f))
        })
        .collect();
    let total = sel.len();
    let total_bytes: u64 = sel.iter().map(|e| e.bytes).sum();
    match p.sort {
        SortKey::Size => {} // the scan keeps its files biggest first
        SortKey::Name => sel.sort_by(|a, b| {
            file_name(a)
                .to_lowercase()
                .cmp(&file_name(b).to_lowercase())
                .then_with(|| a.path.cmp(&b.path))
        }),
        SortKey::Modified => sel.sort_by(|a, b| {
            b.modified
                .cmp(&a.modified)
                .then_with(|| a.path.cmp(&b.path))
        }),
    }
    let page: Vec<Value> = sel
        .iter()
        .skip(p.offset)
        .take(limit)
        .map(|e| entry_json(e))
        .collect();
    Ok(json!({
        "total": total,
        "totalBytes": total_bytes,
        "offset": p.offset,
        "limit": limit,
        "truncated": data.truncated,
        "files": page,
    }))
}

// ---------------------------------------------------------------- tree

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TreeParams {
    scan_id: String,
    path: Option<String>,
}

const MAX_TREE_CHILDREN: usize = 500;

fn tree(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: TreeParams = serde_json::from_value(params)?;
    let scan = get_scan(ctx, &p.scan_id)?;
    let data = lock(&scan);

    let child_json = |node: u32, path: PathBuf| {
        let n = &data.nodes[node as usize];
        json!({
            "path": path.to_string_lossy(),
            "name": &*n.name,
            "bytes": n.sub_bytes,
            "files": n.sub_files,
        })
    };

    // Virtual top level of a multi-root scan.
    if p.path.is_none() && data.roots.len() > 1 {
        let mut kids: Vec<(u64, Value)> = data
            .root_nodes
            .iter()
            .zip(&data.roots)
            .map(|(&n, r)| (data.nodes[n as usize].sub_bytes, child_json(n, r.clone())))
            .collect();
        kids.sort_by_key(|k| std::cmp::Reverse(k.0));
        let bytes: u64 = data
            .root_nodes
            .iter()
            .map(|&n| data.nodes[n as usize].sub_bytes)
            .sum();
        let files: u64 = data
            .root_nodes
            .iter()
            .map(|&n| data.nodes[n as usize].sub_files)
            .sum();
        return Ok(json!({
            "path": Value::Null,
            "name": "",
            "bytes": bytes,
            "files": files,
            "ownBytes": 0,
            "ownFiles": 0,
            "canGoUp": false,
            "parent": Value::Null,
            "childCount": kids.len(),
            "children": kids.into_iter().map(|k| k.1).collect::<Vec<_>>(),
        }));
    }

    let dir: PathBuf = match &p.path {
        Some(s) => walk::require_absolute(s).map_err(ApiError::invalid_params)?,
        None => data.roots[0].clone(),
    };
    let (_, node, exact) = data
        .find_node(&dir)
        .ok_or_else(|| ApiError::not_found("that folder is not part of the scan"))?;
    if !exact {
        return Err(ApiError::not_found(
            "that folder is not part of the scan (or was too deeply nested to be tracked)",
        ));
    }
    let n = &data.nodes[node as usize];
    let node_path = data.node_path(node);
    let mut kids: Vec<(u32, u64)> = n
        .children
        .iter()
        .map(|&c| (c, data.nodes[c as usize].sub_bytes))
        .collect();
    kids.sort_by(|a, b| {
        b.1.cmp(&a.1).then_with(|| {
            data.nodes[a.0 as usize]
                .name
                .cmp(&data.nodes[b.0 as usize].name)
        })
    });
    let child_count = kids.len();
    let children: Vec<Value> = kids
        .into_iter()
        .take(MAX_TREE_CHILDREN)
        .map(|(c, _)| child_json(c, node_path.join(&*data.nodes[c as usize].name)))
        .collect();
    let is_root = n.parent == NO_PARENT;
    let can_go_up = !is_root || data.roots.len() > 1;
    let parent: Value = if is_root {
        Value::Null // the virtual top level (call `tree` without a path)
    } else {
        json!(node_path.parent().map(|p| p.to_string_lossy().into_owned()))
    };
    Ok(json!({
        "path": node_path.to_string_lossy(),
        "name": &*n.name,
        "bytes": n.sub_bytes,
        "files": n.sub_files,
        "ownBytes": n.own_bytes,
        "ownFiles": n.own_files,
        "canGoUp": can_go_up,
        "parent": parent,
        "childCount": child_count,
        "children": children,
    }))
}

// ---------------------------------------------------------------- delete

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteParams {
    scan_id: String,
    paths: Vec<String>,
}

#[derive(Debug, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResult {
    pub path: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub bytes: u64,
}

/// Why `entry` may not be deleted right now, if it may not.
fn precheck(safety: &Safety, entry: &FileEntry) -> std::result::Result<(), String> {
    let path = Path::new(&*entry.path);
    if safety.protected.in_system_tree(path) {
        return Err("refused: the file is inside an operating system folder".into());
    }
    let meta = std::fs::symlink_metadata(path).map_err(|e| io_message(&e))?;
    if !meta.file_type().is_file() {
        return Err("no longer a regular file; run the analysis again".into());
    }
    if meta.len() != entry.bytes || walk::mtime_secs(&meta) != entry.modified {
        return Err("the file changed since the analysis; run it again".into());
    }
    Ok(())
}

fn delete(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: DeleteParams = serde_json::from_value(params)?;
    if p.paths.is_empty() {
        return Err(ApiError::invalid_params("`paths` is empty"));
    }
    let scan = get_scan(ctx, &p.scan_id)?;
    let mut data = lock(&scan);

    let s = settings::load(ctx);
    let excludes = ExcludeSet::from_settings(&ctx.env, &s)?;
    let safety = Arc::new(Safety::new(&ctx.env, excludes));
    let secure = s.secure_delete.enabled.then_some(s.secure_delete.passes);

    let by_path: HashMap<&str, &FileEntry> = data.files.iter().map(|e| (&*e.path, e)).collect();
    let mut deleters: HashMap<u16, std::result::Result<SafeDeleter, String>> = HashMap::new();
    let mut results: Vec<DeleteResult> = Vec::new();
    let mut removed: HashSet<Box<str>> = HashSet::new();
    let mut freed = 0u64;
    let mut seen: HashSet<&str> = HashSet::new();
    let total = p.paths.len();

    for (i, raw) in p.paths.iter().enumerate() {
        if !seen.insert(raw.as_str()) {
            continue; // repeated in the request
        }
        let fail = |msg: &str| DeleteResult {
            path: raw.clone(),
            ok: false,
            error: Some(msg.to_string()),
            bytes: 0,
        };
        if job.is_cancelled() {
            results.push(fail("cancelled"));
            continue;
        }
        job.progress(
            ProgressEvent::new("delete")
                .fraction(i as f64 / total as f64)
                .counts(i as u64, total as u64)
                .message(raw.clone()),
        );
        let Some(entry) = by_path.get(raw.as_str()).copied() else {
            results.push(fail("not part of this analysis"));
            continue;
        };
        if let Err(msg) = precheck(&safety, entry) {
            results.push(fail(&msg));
            continue;
        }
        let deleter = deleters.entry(entry.root).or_insert_with(|| {
            // The scanned root is the allowed base; it may itself be a protected
            // folder (a drive, the home directory), each file is still vetted.
            SafeDeleter::for_selection(safety.clone(), &data.roots[entry.root as usize])
                .map(|d| match secure {
                    Some(passes) => d.with_secure(passes),
                    None => d,
                })
                .map_err(|e| e.message)
        });
        let deleter = match deleter {
            Ok(d) => d,
            Err(msg) => {
                results.push(fail(&format!("cannot delete inside this folder: {msg}")));
                continue;
            }
        };
        match deleter.remove_file(Path::new(&*entry.path)) {
            Ok(bytes) => {
                freed += bytes;
                removed.insert(entry.path.clone());
                results.push(DeleteResult {
                    path: raw.clone(),
                    ok: true,
                    error: None,
                    bytes,
                });
            }
            Err(e) if e.is_not_found() => {
                // Already gone: forget it, but do not claim we freed anything.
                removed.insert(entry.path.clone());
                results.push(DeleteResult {
                    path: raw.clone(),
                    ok: true,
                    error: None,
                    bytes: 0,
                });
            }
            Err(e) => results.push(fail(&e.message())),
        }
    }
    drop(by_path);
    let deleted = results.iter().filter(|r| r.ok && r.bytes > 0).count();
    data.apply_removed(&removed);
    Ok(json!({
        "results": results,
        "deleted": deleted,
        "freedBytes": freed,
        "totals": totals_json(&data),
        "totalFiles": data.total_files(),
        "totalBytes": data.total_bytes(),
    }))
}

// ---------------------------------------------------------------- open_folder

#[derive(Debug, Deserialize)]
struct OpenParams {
    path: String,
}

fn open_folder(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: OpenParams = serde_json::from_value(params)?;
    let path = walk::require_absolute(&p.path).map_err(ApiError::invalid_params)?;
    let meta = std::fs::metadata(&path).map_err(|e| {
        ApiError::from(std::io::Error::new(
            e.kind(),
            format!("{}: {}", path.display(), io_message(&e)),
        ))
    })?;
    let is_dir = meta.is_dir();
    let arg = path.to_string_lossy().into_owned();
    let (program, args, ignore_status, opened): (&str, Vec<String>, bool, PathBuf) =
        match ctx.env.os {
            // Never open the file itself: that would run it.
            Os::Linux => {
                let target = if is_dir {
                    path.clone()
                } else {
                    path.parent().unwrap_or(&path).to_path_buf()
                };
                (
                    "xdg-open",
                    vec![target.to_string_lossy().into_owned()],
                    false,
                    target,
                )
            }
            Os::MacOs if is_dir => ("open", vec![arg], false, path.clone()),
            Os::MacOs => ("open", vec!["-R".into(), arg], false, path.clone()),
            // explorer.exe reports failure (exit code 1) even when it worked.
            Os::Windows if is_dir => ("explorer", vec![arg], true, path.clone()),
            Os::Windows => (
                "explorer",
                vec![format!("/select,{arg}")],
                true,
                path.clone(),
            ),
        };
    if ctx.runner.which(program).is_none() {
        return Err(ApiError::unsupported(format!(
            "`{program}` is not available, so the folder cannot be opened"
        )));
    }
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = ctx.runner.run(program, &argv)?;
    if !out.success() && !ignore_status {
        return Err(ApiError::io(format!(
            "`{program}` failed: {}",
            out.stderr.trim()
        )));
    }
    Ok(json!({"opened": opened.to_string_lossy()}))
}
