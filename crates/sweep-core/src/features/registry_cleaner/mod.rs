//! Registry cleaner (Windows) / "Config Issues" (Linux, macOS): find broken settings and repair
//! them safely.
//!
//! Methods:
//! - `registry_cleaner.categories`: the categories this system can scan, and the page title.
//! - `registry_cleaner.scan { categories? }`: `{ issues, counts, ... }`. Read-only. Windows: 15
//!   registry categories behind [`regaccess::RegistryAccess`]; Linux: launchers, autostart,
//!   dangling links, `mimeapps.list`, user services, orphaned packages; macOS: launch agents
//!   and dangling links.
//! - `registry_cleaner.fix { issueIds, backup: true }`: scans again, refuses any id that is not
//!   in that fresh scan, backs everything up (mandatory; any backup failure aborts before a
//!   single change), then fixes. Machine-wide registry keys are changed in one elevated batch.
//! - `registry_cleaner.list_backups`, `registry_cleaner.restore_backup { id }`,
//!   `registry_cleaner.delete_backup { id }`.
//!
//! Every scanner is deliberately conservative: an item that cannot be judged with certainty is
//! never reported (see [`cmdline`] for the Windows rules).

use serde::Deserialize;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use crate::api::Registry;
use crate::ctx::{Ctx, Os};
use crate::error::{ApiError, ErrorCode, Result};
use crate::job::{Job, ProgressEvent};

pub mod backup;
pub mod cmdline;
pub mod desktop;
pub mod fix;
pub mod mimeapps;
pub mod model;
pub mod regaccess;
pub mod regcmd;
pub mod unixscan;
pub mod winscan;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

use model::{categories_for, CategoryInfo, Found};
use regaccess::RegistryAccess;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "registry_cleaner.categories",
    "registry_cleaner.scan",
    "registry_cleaner.fix",
    "registry_cleaner.list_backups",
    "registry_cleaner.restore_backup",
    "registry_cleaner.delete_backup",
];

pub fn register(r: &mut Registry) {
    r.add("registry_cleaner.categories", categories_handler);
    r.add("registry_cleaner.scan", scan_handler);
    r.add("registry_cleaner.fix", fix_handler);
    r.add("registry_cleaner.list_backups", list_backups_handler);
    r.add("registry_cleaner.restore_backup", restore_backup_handler);
    r.add("registry_cleaner.delete_backup", delete_backup_handler);
}

// ---------------------------------------------------------------- registry access

type RegHandle = Rc<dyn RegistryAccess>;

thread_local! {
    static REG_OVERRIDE: RefCell<Option<RegHandle>> = const { RefCell::new(None) };
}

/// Run `f` with `reg` standing in for the Windows registry on this thread (tests only).
#[cfg(test)]
pub fn with_registry<T>(reg: RegHandle, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<RegHandle>);
    impl Drop for Restore {
        fn drop(&mut self) {
            REG_OVERRIDE.with(|r| *r.borrow_mut() = self.0.take());
        }
    }
    let prev = REG_OVERRIDE.with(|r| r.borrow_mut().replace(reg));
    let _restore = Restore(prev);
    f()
}

fn registry_handle() -> Result<RegHandle> {
    if let Some(r) = REG_OVERRIDE.with(|r| r.borrow().clone()) {
        return Ok(r);
    }
    #[cfg(windows)]
    {
        Ok(Rc::new(regaccess::RealRegistry))
    }
    #[cfg(not(windows))]
    {
        Err(ApiError::unsupported(
            "The Windows registry is not available on this system",
        ))
    }
}

// ---------------------------------------------------------------- scanning

/// A category that could not be scanned (the rest still are).
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct SkippedCategory {
    pub category: String,
    pub reason: String,
}

pub struct ScanOutput {
    pub found: Vec<Found>,
    pub scanned: Vec<&'static str>,
    pub skipped: Vec<SkippedCategory>,
}

fn title_for(os: Os) -> &'static str {
    match os {
        Os::Windows => "Registry",
        _ => "Config Issues",
    }
}

fn resolve_categories(ctx: &Ctx, wanted: Option<&[String]>) -> Result<Vec<&'static CategoryInfo>> {
    let all = categories_for(ctx.env.os);
    let Some(w) = wanted else {
        return Ok(all.iter().collect());
    };
    if w.is_empty() {
        return Err(ApiError::invalid_params("`categories` must not be empty"));
    }
    let mut out = Vec::new();
    for id in w {
        let c = all
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| ApiError::invalid_params(format!("unknown category `{id}`")))?;
        if !out.iter().any(|x: &&CategoryInfo| x.id == c.id) {
            out.push(c);
        }
    }
    Ok(out)
}

pub fn scan_categories(ctx: &Ctx, cats: &[&'static CategoryInfo], job: &Job) -> Result<ScanOutput> {
    let reg = if ctx.env.os == Os::Windows {
        Some(registry_handle()?)
    } else {
        None
    };
    let mut out = ScanOutput {
        found: Vec::new(),
        scanned: Vec::new(),
        skipped: Vec::new(),
    };
    for (i, c) in cats.iter().enumerate() {
        job.check_cancelled()?;
        job.progress(
            ProgressEvent::new("scan")
                .message(format!("Checking {}", c.label.to_lowercase()))
                .counts(i as u64, cats.len() as u64)
                .fraction(i as f64 / cats.len().max(1) as f64),
        );
        let r = match &reg {
            Some(reg) => winscan::scan_category(reg.as_ref(), c.id, job),
            None => unixscan::scan_category(ctx, c.id, job),
        };
        match r {
            Ok(mut v) => {
                v.sort_by(|a, b| {
                    a.issue
                        .location
                        .cmp(&b.issue.location)
                        .then(a.issue.value.cmp(&b.issue.value))
                });
                out.found.extend(v);
                out.scanned.push(c.id);
            }
            Err(e) if e.code == ErrorCode::Cancelled => return Err(e),
            Err(e) => out.skipped.push(SkippedCategory {
                category: c.id.to_string(),
                reason: e.message,
            }),
        }
    }
    // The same registry item can be reachable twice (merged views); keep the first.
    let mut seen = HashSet::new();
    out.found.retain(|f| seen.insert(f.issue.id.clone()));
    Ok(out)
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ScanParams {
    categories: Option<Vec<String>>,
}

fn categories_handler(ctx: &Ctx, _params: Value, _job: &Job) -> Result<Value> {
    let os = ctx.env.os;
    Ok(json!({
        "platform": backup::platform_name(os),
        "title": title_for(os),
        "categories": categories_for(os),
    }))
}

fn scan_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: ScanParams = if params.is_null() {
        ScanParams::default()
    } else {
        serde_json::from_value(params)?
    };
    let cats = resolve_categories(ctx, p.categories.as_deref())?;
    let out = scan_categories(ctx, &cats, job)?;
    let mut counts: BTreeMap<&str, usize> = out.scanned.iter().map(|c| (*c, 0)).collect();
    for f in &out.found {
        if let Some(n) = counts.get_mut(f.issue.category.as_str()) {
            *n += 1;
        }
    }
    let os = ctx.env.os;
    Ok(json!({
        "platform": backup::platform_name(os),
        "title": title_for(os),
        "issues": out.found.iter().map(|f| &f.issue).collect::<Vec<_>>(),
        "counts": counts,
        "scanned": out.scanned,
        "skipped": out.skipped,
    }))
}

// ---------------------------------------------------------------- fixing

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixParams {
    issue_ids: Vec<String>,
    /// Must be `true` (or absent): there is no way to fix without a backup.
    #[serde(default = "yes")]
    backup: bool,
    #[serde(default)]
    categories: Option<Vec<String>>,
}

fn yes() -> bool {
    true
}

/// Sanity limit on one fix request.
const MAX_IDS: usize = 50_000;

fn fix_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: FixParams = serde_json::from_value(params)?;
    if !p.backup {
        return Err(ApiError::invalid_params(
            "A backup is mandatory: `backup` must be true",
        ));
    }
    if p.issue_ids.is_empty() {
        return Err(ApiError::invalid_params("`issueIds` is empty"));
    }
    if p.issue_ids.len() > MAX_IDS {
        return Err(ApiError::invalid_params("too many issues in one request"));
    }
    let cats = resolve_categories(ctx, p.categories.as_deref())?;
    // Ids only count if a scan made right now finds them again.
    let scan = scan_categories(ctx, &cats, job)?;
    let outcome = fix::apply(ctx, scan.found, &p.issue_ids, job)?;
    Ok(serde_json::to_value(outcome)?)
}

// ---------------------------------------------------------------- backups

fn list_backups_handler(ctx: &Ctx, _params: Value, _job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(backup::list(ctx))?)
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

fn restore_backup_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: IdParams = serde_json::from_value(params)?;
    Ok(serde_json::to_value(backup::restore(ctx, &p.id, job)?)?)
}

fn delete_backup_handler(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: IdParams = serde_json::from_value(params)?;
    if !backup::valid_backup_id(&p.id) {
        return Err(ApiError::invalid_params(format!(
            "`{}` is not a backup id",
            p.id
        )));
    }
    let freed = backup::delete_backup_entry(ctx, &p.id)?;
    Ok(json!({ "id": p.id, "freedBytes": freed }))
}
