//! Duplicate finder: locate files that are copies of each other, choose which copies
//! to remove, and delete them without ever removing the last copy.
//!
//! Methods (JSON, camelCase):
//! - `duplicates.scan { paths, excludePaths?, matchBy: {name,size,modified,content},
//!   minSize?, maxSize?, includeHidden=false, includeSystem=false, skipZeroByte=true,
//!   followLinks=false }` -> `{scanId, groups: [{groupId, key, files, wastedBytes}],
//!   totalGroups, totalFiles, wastedBytes, scannedFiles, errors, durationMs}`
//! - `duplicates.groups { scanId, offset, limit }` -> further groups of a scan
//! - `duplicates.auto_select { scanId, rule, folder? }` -> paths to remove per group; at
//!   least one file per group is always left out
//! - `duplicates.delete { scanId, paths }` -> per-path results
//! - `duplicates.export { scanId, format: "csv"|"txt" }` -> `{format, filename, text}`
//!
//! Content matching narrows by size, then a 4 KiB head + 4 KiB tail hash, then a full
//! streaming BLAKE3. Several names of one file (hard links) count as ONE file, and
//! symbolic links are never candidates, so a file is never reported as a duplicate of
//! itself. A walk stays on the filesystem of each scanned folder.

pub mod engine;
#[cfg(test)]
mod tests;

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::api::Registry;
use crate::ctx::{Ctx, Os};
use crate::error::{ApiError, Result};
use crate::features::disk_analyzer::cache::ScanStore;
use crate::features::disk_analyzer::walk::{mtime_secs, require_absolute};
use crate::features::settings;
use crate::fsutil::random_id;
use crate::job::{Job, ProgressEvent};
use crate::safety::{self, io_message, is_within, ExcludeSet, SafeDeleter, Safety};

use engine::{Group, MatchBy, Options};

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "duplicates.scan",
    "duplicates.groups",
    "duplicates.auto_select",
    "duplicates.delete",
    "duplicates.export",
];

pub fn register(r: &mut Registry) {
    r.add("duplicates.scan", scan);
    r.add("duplicates.groups", groups);
    r.add("duplicates.auto_select", auto_select);
    r.add("duplicates.delete", delete);
    r.add("duplicates.export", export);
}

/// Groups returned inline by `duplicates.scan`; the rest come from `duplicates.groups`.
const INLINE_GROUPS: usize = 2000;
pub const MAX_SCANS: usize = 3;

pub struct DupScan {
    id: String,
    roots: Vec<PathBuf>,
    groups: Vec<Group>,
    match_by: MatchBy,
}

static SCANS: ScanStore<DupScan> = ScanStore::new(MAX_SCANS);

fn get_scan(ctx: &Ctx, id: &str) -> Result<Arc<Mutex<DupScan>>> {
    SCANS
        .get(&ctx.env.data_dir, id)
        .ok_or_else(|| ApiError::not_found("unknown or expired scan; run the search again"))
}

fn lock(s: &Mutex<DupScan>) -> std::sync::MutexGuard<'_, DupScan> {
    s.lock().unwrap_or_else(|p| p.into_inner())
}

fn totals(groups: &[Group]) -> (usize, usize, u64) {
    (
        groups.len(),
        groups.iter().map(|g| g.files.len()).sum(),
        groups.iter().map(|g| g.wasted_bytes).sum(),
    )
}

// ---------------------------------------------------------------- scan

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanParams {
    paths: Vec<String>,
    #[serde(default)]
    exclude_paths: Vec<String>,
    match_by: MatchBy,
    min_size: Option<u64>,
    max_size: Option<u64>,
    #[serde(default)]
    include_hidden: bool,
    #[serde(default)]
    include_system: bool,
    #[serde(default = "default_true")]
    skip_zero_byte: bool,
    #[serde(default)]
    follow_links: bool,
}

fn scan(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let start = Instant::now();
    let p: ScanParams = serde_json::from_value(params)?;
    let m = p.match_by;
    if !(m.name || m.size || m.modified || m.content) {
        return Err(ApiError::invalid_params(
            "choose at least one thing to match by (name, size, date or content)",
        ));
    }
    if let (Some(lo), Some(hi)) = (p.min_size, p.max_size) {
        if lo > hi {
            return Err(ApiError::invalid_params(
                "`minSize` is larger than `maxSize`",
            ));
        }
    }
    if p.paths.is_empty() {
        return Err(ApiError::invalid_params("`paths` is empty"));
    }
    let protected = safety::Protected::new(&ctx.env);
    let mut roots: Vec<PathBuf> = Vec::new();
    for raw in &p.paths {
        let path = require_absolute(raw).map_err(ApiError::invalid_params)?;
        let canon = safety::canonicalize(&path).map_err(|e| {
            ApiError::from(std::io::Error::new(
                e.kind(),
                format!("{raw}: {}", io_message(&e)),
            ))
        })?;
        if !canon.is_dir() {
            return Err(ApiError::invalid_params(format!("not a folder: {raw}")));
        }
        if !p.include_system && protected.in_system_tree(&canon) {
            return Err(ApiError::invalid_params(format!(
                "{raw} is an operating system folder; enable \"include system files\" to search it"
            )));
        }
        roots.push(canon);
    }
    roots.sort_by_key(|r| r.components().count());
    let mut unique: Vec<PathBuf> = Vec::new();
    for r in roots {
        if !unique.iter().any(|o| is_within(&r, o)) {
            unique.push(r);
        }
    }

    // The user's global exclusion list applies to the search too, so an excluded file is
    // never offered for deletion. Fails closed on a bad pattern.
    let s = settings::load(ctx);
    let mut patterns: Vec<String> = s.exclude.iter().map(|e| e.pattern.clone()).collect();
    patterns.extend(p.exclude_paths.iter().cloned());
    let excludes = ExcludeSet::from_patterns(&ctx.env, &patterns)?;

    let opts = Options {
        match_by: m,
        min_size: p.min_size,
        max_size: p.max_size,
        include_hidden: p.include_hidden,
        include_system: p.include_system,
        skip_zero_byte: p.skip_zero_byte,
        follow_links: p.follow_links,
        case_insensitive_names: ctx.env.os != Os::Linux,
    };
    let found = engine::find(ctx, &unique, &excludes, &opts, job)?;

    let (n_groups, n_files, wasted) = totals(&found.groups);
    let inline: Vec<&Group> = found.groups.iter().take(INLINE_GROUPS).collect();
    let id = format!("dup-{}", random_id());
    let out = json!({
        "scanId": id,
        "groups": inline,
        "totalGroups": n_groups,
        "totalFiles": n_files,
        "wastedBytes": wasted,
        "scannedFiles": found.scanned_files,
        "truncatedGroups": n_groups > INLINE_GROUPS,
        "errors": {"count": found.error_count, "samples": found.error_samples},
        "durationMs": start.elapsed().as_millis() as u64,
    });
    SCANS.insert(
        &ctx.env.data_dir,
        id.clone(),
        DupScan {
            id,
            roots: unique,
            groups: found.groups,
            match_by: m,
        },
    );
    Ok(out)
}

// ---------------------------------------------------------------- groups

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupsParams {
    scan_id: String,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

fn groups(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: GroupsParams = serde_json::from_value(params)?;
    let scan = get_scan(ctx, &p.scan_id)?;
    let data = lock(&scan);
    let limit = p.limit.unwrap_or(500).clamp(1, 2000);
    let (n_groups, n_files, wasted) = totals(&data.groups);
    let page: Vec<&Group> = data.groups.iter().skip(p.offset).take(limit).collect();
    Ok(json!({
        "groups": page,
        "offset": p.offset,
        "totalGroups": n_groups,
        "totalFiles": n_files,
        "wastedBytes": wasted,
    }))
}

// ---------------------------------------------------------------- auto select

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    KeepNewest,
    KeepOldest,
    KeepShortestPath,
    KeepInFolder,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoParams {
    scan_id: String,
    rule: Rule,
    folder: Option<String>,
}

/// Paths of `group` to remove under `rule`. Ties are broken by the shorter, then the
/// alphabetically first path, so the outcome is deterministic. At least one file is
/// always left out; a group the rule cannot decide (`KeepInFolder` with no copy inside the
/// folder) yields nothing.
pub fn select_in_group(group: &Group, rule: Rule, folder: Option<&Path>) -> Vec<String> {
    let files = &group.files;
    let tie = |a: &engine::DupFile, b: &engine::DupFile| {
        a.path
            .chars()
            .count()
            .cmp(&b.path.chars().count())
            .then_with(|| a.path.cmp(&b.path))
    };
    let selected: Vec<String> = match rule {
        Rule::KeepNewest | Rule::KeepOldest | Rule::KeepShortestPath => {
            let keeper = files.iter().min_by(|a, b| match rule {
                Rule::KeepNewest => b.modified.cmp(&a.modified).then_with(|| tie(a, b)),
                Rule::KeepOldest => a.modified.cmp(&b.modified).then_with(|| tie(a, b)),
                _ => tie(a, b).then_with(|| a.modified.cmp(&b.modified)),
            });
            files
                .iter()
                .filter(|f| keeper.is_some_and(|k| k.path != f.path))
                .map(|f| f.path.clone())
                .collect()
        }
        Rule::KeepInFolder => {
            let Some(folder) = folder else {
                return Vec::new();
            };
            if !files.iter().any(|f| is_within(Path::new(&f.path), folder)) {
                return Vec::new();
            }
            files
                .iter()
                .filter(|f| !is_within(Path::new(&f.path), folder))
                .map(|f| f.path.clone())
                .collect()
        }
    };
    if selected.len() >= files.len() {
        return Vec::new(); // never select every member
    }
    selected
}

fn auto_select(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: AutoParams = serde_json::from_value(params)?;
    let folder = match (p.rule, &p.folder) {
        (Rule::KeepInFolder, None) => {
            return Err(ApiError::invalid_params(
                "`folder` is required for the keep_in_folder rule",
            ))
        }
        (_, Some(f)) => Some(require_absolute(f).map_err(ApiError::invalid_params)?),
        _ => None,
    };
    let scan = get_scan(ctx, &p.scan_id)?;
    let data = lock(&scan);
    let mut out = Vec::new();
    let (mut count, mut bytes) = (0usize, 0u64);
    for g in &data.groups {
        let sel = select_in_group(g, p.rule, folder.as_deref());
        if sel.is_empty() {
            continue;
        }
        let chosen: HashSet<&str> = sel.iter().map(String::as_str).collect();
        count += sel.len();
        bytes += g
            .files
            .iter()
            .filter(|f| chosen.contains(f.path.as_str()))
            .map(|f| f.bytes)
            .sum::<u64>();
        out.push(json!({"groupId": g.group_id, "selected": sel}));
    }
    Ok(json!({"groups": out, "count": count, "bytes": bytes}))
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

/// Is the file still a regular file with the size and modification time the scan saw?
fn unchanged(f: &engine::DupFile) -> bool {
    match fs::symlink_metadata(&f.path) {
        Ok(m) => m.file_type().is_file() && m.len() == f.bytes && mtime_secs(&m) == f.modified,
        Err(_) => false,
    }
}

/// Current full hash of a file, or why it could not be computed.
fn current_hash(path: &str, job: &Job) -> std::result::Result<String, String> {
    match engine::hash_file(Path::new(path), job, |_| {}) {
        Ok(h) => Ok(h.to_hex().to_string()),
        Err(engine::HashError::Cancelled) => Err("cancelled".into()),
        Err(engine::HashError::Io(e)) => Err(io_message(&e)),
    }
}

fn root_of<'a>(roots: &'a [PathBuf], path: &str) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|r| is_within(Path::new(path), r))
        .max_by_key(|r| r.components().count())
}

fn delete(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: DeleteParams = serde_json::from_value(params)?;
    if p.paths.is_empty() {
        return Err(ApiError::invalid_params("`paths` is empty"));
    }
    let scan = get_scan(ctx, &p.scan_id)?;
    let mut data = lock(&scan);

    // ---- validate the whole request before touching anything
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (gi, g) in data.groups.iter().enumerate() {
        for f in &g.files {
            index.insert(f.path.as_str(), gi);
        }
    }
    let mut seen: HashSet<&str> = HashSet::new();
    let mut per_group: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
    for raw in &p.paths {
        if !seen.insert(raw.as_str()) {
            continue;
        }
        let Some(&gi) = index.get(raw.as_str()) else {
            return Err(ApiError::invalid_params(format!(
                "{raw} is not part of this search"
            )));
        };
        per_group.entry(gi).or_default().push(raw.as_str());
    }
    for (gi, sel) in &per_group {
        let g = &data.groups[*gi];
        if sel.len() >= g.files.len() {
            return Err(ApiError::invalid_params(format!(
                "this would delete every copy in group {} ({}); keep at least one file of each group",
                g.group_id, g.files[0].path
            )));
        }
    }

    // ---- delete group by group, re-verifying against a surviving copy
    let s = settings::load(ctx);
    let excludes = ExcludeSet::from_settings(&ctx.env, &s)?;
    let safety = Arc::new(Safety::new(&ctx.env, excludes));
    let secure = s.secure_delete.enabled.then_some(s.secure_delete.passes);
    let content = data.match_by.content;
    let mut deleters: HashMap<PathBuf, std::result::Result<SafeDeleter, String>> = HashMap::new();
    let mut results: Vec<DeleteResult> = Vec::new();
    let mut removed: HashSet<String> = HashSet::new();
    let mut freed = 0u64;
    let total = seen.len();
    let mut done = 0usize;

    for (gi, sel) in &per_group {
        let g = &data.groups[*gi];
        let victims: Vec<&engine::DupFile> = g
            .files
            .iter()
            .filter(|f| sel.contains(&f.path.as_str()))
            .collect();
        let fail_all = |msg: &str, results: &mut Vec<DeleteResult>| {
            for v in &victims {
                results.push(DeleteResult {
                    path: v.path.clone(),
                    ok: false,
                    error: Some(msg.to_string()),
                    bytes: 0,
                });
            }
        };

        // A copy that is guaranteed to remain must still be there and still be equal.
        let survivor_hash: Option<String> = if content {
            let want = g.key.hash.as_deref().unwrap_or_default();
            g.files
                .iter()
                .filter(|f| !sel.contains(&f.path.as_str()))
                .find_map(|f| {
                    let m = fs::symlink_metadata(&f.path).ok()?;
                    if !m.file_type().is_file() || m.len() != f.bytes {
                        return None;
                    }
                    current_hash(&f.path, job).ok().filter(|h| h == want)
                })
        } else {
            g.files
                .iter()
                .filter(|f| !sel.contains(&f.path.as_str()))
                .any(unchanged)
                .then(String::new)
        };
        if job.is_cancelled() {
            fail_all("cancelled", &mut results);
            continue;
        }
        let Some(survivor_hash) = survivor_hash else {
            fail_all(
                "none of the copies to keep is still unchanged; run the search again",
                &mut results,
            );
            continue;
        };

        for v in victims {
            done += 1;
            job.progress(
                ProgressEvent::new("delete")
                    .fraction(done as f64 / total.max(1) as f64)
                    .counts(done as u64, total as u64)
                    .message(v.path.clone()),
            );
            let fail = |msg: String| DeleteResult {
                path: v.path.clone(),
                ok: false,
                error: Some(msg),
                bytes: 0,
            };
            if job.is_cancelled() {
                results.push(fail("cancelled".into()));
                continue;
            }
            let path = Path::new(&v.path);
            if safety.protected.in_system_tree(path) {
                results.push(fail(
                    "refused: the file is inside an operating system folder".into(),
                ));
                continue;
            }
            if !unchanged(v) {
                results.push(fail(
                    "the file changed since the search; run it again".into(),
                ));
                continue;
            }
            if content {
                match current_hash(&v.path, job) {
                    Ok(h) if h == survivor_hash => {}
                    Ok(_) => {
                        results.push(fail(
                            "the content differs from the copy being kept; run the search again"
                                .into(),
                        ));
                        continue;
                    }
                    Err(msg) => {
                        results.push(fail(msg));
                        continue;
                    }
                }
            }
            let Some(root) = root_of(&data.roots, &v.path) else {
                results.push(fail("not inside a searched folder".into()));
                continue;
            };
            let deleter = deleters.entry(root.clone()).or_insert_with(|| {
                SafeDeleter::for_selection(safety.clone(), root)
                    .map(|d| match secure {
                        Some(passes) => d.with_secure(passes),
                        None => d,
                    })
                    .map_err(|e| e.message)
            });
            let deleter = match deleter {
                Ok(d) => d,
                Err(msg) => {
                    results.push(fail(format!("cannot delete inside this folder: {msg}")));
                    continue;
                }
            };
            match deleter.remove_file(path) {
                Ok(bytes) => {
                    freed += bytes;
                    removed.insert(v.path.clone());
                    results.push(DeleteResult {
                        path: v.path.clone(),
                        ok: true,
                        error: None,
                        bytes,
                    });
                }
                Err(e) => results.push(fail(e.message())),
            }
        }
    }

    // ---- forget what is gone; a group with one copy left is no longer a duplicate
    for g in data.groups.iter_mut() {
        g.files.retain(|f| !removed.contains(&f.path));
        g.wasted_bytes = engine::wasted_of(&g.files);
    }
    data.groups.retain(|g| g.files.len() > 1);
    let (n_groups, n_files, wasted) = totals(&data.groups);
    let deleted = results.iter().filter(|r| r.ok).count();
    Ok(json!({
        "results": results,
        "deleted": deleted,
        "freedBytes": freed,
        "remainingGroups": n_groups,
        "remainingFiles": n_files,
        "wastedBytes": wasted,
    }))
}

// ---------------------------------------------------------------- export

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ExportFormat {
    Csv,
    Txt,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportParams {
    scan_id: String,
    format: ExportFormat,
}

fn when(secs: i64) -> String {
    OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// CSV: `group,wasted_bytes,path,bytes,modified` (one row per file, RFC 4180 quoting).
pub fn render_csv(groups: &[Group]) -> String {
    let mut out = String::from("group,wasted_bytes,path,bytes,modified\r\n");
    for (i, g) in groups.iter().enumerate() {
        for f in &g.files {
            out.push_str(&format!(
                "{},{},{},{},{}\r\n",
                i + 1,
                g.wasted_bytes,
                csv_field(&f.path),
                f.bytes,
                when(f.modified)
            ));
        }
    }
    out
}

/// Plain text: one block per group.
pub fn render_txt(groups: &[Group]) -> String {
    let mut out = String::new();
    let wasted: u64 = groups.iter().map(|g| g.wasted_bytes).sum();
    out.push_str(&format!(
        "ClearSweep duplicate report: {} groups, {} bytes wasted\n\n",
        groups.len(),
        wasted
    ));
    for (i, g) in groups.iter().enumerate() {
        out.push_str(&format!(
            "Group {}: {} files, {} bytes wasted\n",
            i + 1,
            g.files.len(),
            g.wasted_bytes
        ));
        for f in &g.files {
            out.push_str(&format!(
                "  {}  ({} bytes, modified {})\n",
                f.path,
                f.bytes,
                when(f.modified)
            ));
        }
        out.push('\n');
    }
    out
}

fn export(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: ExportParams = serde_json::from_value(params)?;
    let scan = get_scan(ctx, &p.scan_id)?;
    let data = lock(&scan);
    let (text, ext) = match p.format {
        ExportFormat::Csv => (render_csv(&data.groups), "csv"),
        ExportFormat::Txt => (render_txt(&data.groups), "txt"),
    };
    Ok(json!({
        "format": ext,
        "filename": format!("clearsweep-duplicates.{ext}"),
        "text": text,
        "id": data.id,
    }))
}
