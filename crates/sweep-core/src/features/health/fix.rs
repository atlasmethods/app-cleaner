//! `health.fix`: performs ONLY what the caller lists. Nothing is decided here: the UI passes
//! the selections the user reviewed, and every id is looked up again on the server (an id only
//! *selects*; a stale, unknown or unsafe id is refused and reported, never acted on).

use serde_json::{json, Value};
use std::collections::HashSet;

use crate::api::dispatch;
use crate::ctx::Ctx;
use crate::error::{ApiError, ErrorCode, Result};
use crate::features::cleaner::{self, CleanOptions, CleanReport, Skipped, Source};
use crate::features::optimizer;
use crate::features::settings::{self, CloseBrowsers};
use crate::features::startup::{self, ToggleOpts};
use crate::job::{Job, ProgressEvent};

use super::analyze::{self, background_apps, fmt_bytes, plural, scoped, startup_candidate};
use super::model::{FixReport, ItemResult, Part, PartResult, PartStatus};
use super::store;

/// What to do. Everything is opt-in; an empty request is refused by the handler.
#[derive(Debug, Clone, Default)]
pub struct FixRequest {
    pub privacy: bool,
    pub space: bool,
    pub startup_ids: Vec<String>,
    pub sleep_app_ids: Vec<String>,
    pub update_ids: Vec<String>,
    /// What to do with rules whose browser is running. The Health Check is interactive, so
    /// `Ask` behaves like `Skip` here (the result lists the blocked apps for the UI to ask).
    pub close_apps: CloseBrowsers,
}

impl FixRequest {
    pub fn parts(&self) -> Vec<Part> {
        let mut v = Vec::new();
        if self.privacy {
            v.push(Part::Privacy);
        }
        if self.space {
            v.push(Part::Space);
        }
        if !self.startup_ids.is_empty() {
            v.push(Part::Startup);
        }
        if !self.sleep_app_ids.is_empty() {
            v.push(Part::Sleep);
        }
        if !self.update_ids.is_empty() {
            v.push(Part::Updates);
        }
        v
    }
}

fn label(part: Part) -> &'static str {
    match part {
        Part::Privacy => "Removing tracking data",
        Part::Space => "Deleting junk files",
        Part::Startup => "Switching off startup items",
        Part::Sleep => "Putting apps to sleep",
        Part::Updates => "Installing updates",
    }
}

fn dedupe(ids: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.iter().filter(|i| seen.insert((*i).clone())).cloned().collect()
}

// ---------------------------------------------------------------- privacy / space

fn describe_removed(bytes: u64, files: u64, rows: u64) -> String {
    let mut parts = Vec::new();
    if bytes > 0 || files > 0 {
        parts.push(format!("{} in {}", fmt_bytes(bytes), plural(files, "file", "files")));
    }
    if rows > 0 {
        parts.push(plural(rows, "database entry", "database entries"));
    }
    if parts.is_empty() {
        "nothing".to_string()
    } else {
        parts.join(" and ")
    }
}

fn clean_part(
    ctx: &Ctx,
    part: Part,
    ids: Vec<String>,
    close: CloseBrowsers,
    job: &Job,
) -> Result<PartResult> {
    if ids.is_empty() {
        return Ok(PartResult::new(
            part,
            PartStatus::Done,
            "Nothing is selected for cleaning on the Clean tab.",
        ));
    }
    let rep: CleanReport = cleaner::run_clean(
        ctx,
        Some(ids),
        &CleanOptions::new(close),
        Source::Health,
        job,
    )?;
    let mut blocked: Vec<String> = Vec::new();
    let mut in_use = 0u64;
    let mut failed = 0u64;
    for r in &rep.results {
        match r.skipped {
            Some(Skipped::AppRunning) => {
                for a in &r.running_apps {
                    if !blocked.contains(a) {
                        blocked.push(a.clone());
                    }
                }
            }
            Some(Skipped::InUse) => in_use += 1,
            _ => {}
        }
        failed += r.failed.len() as u64;
    }
    let removed = describe_removed(rep.total_bytes, rep.total_files, rep.total_rows);
    let mut notes = Vec::new();
    if !blocked.is_empty() {
        notes.push(format!("{} still running, so its data was left alone", blocked.join(", ")));
    }
    if in_use > 0 {
        notes.push(format!("{} in use", plural(in_use, "item was", "items were")));
    }
    if failed > 0 {
        notes.push(format!("{} could not be removed", plural(failed, "item", "items")));
    }
    let something = rep.total_bytes > 0 || rep.total_files > 0 || rep.total_rows > 0;
    let status = if notes.is_empty() {
        PartStatus::Done
    } else if something {
        PartStatus::Partial
    } else {
        PartStatus::Failed
    };
    let message = if notes.is_empty() {
        format!("Removed {removed}.")
    } else {
        format!("Removed {removed}; {}.", notes.join("; "))
    };
    let mut p = PartResult::new(part, status, message);
    p.removed_bytes = rep.total_bytes;
    p.removed_files = rep.total_files;
    p.removed_rows = rep.total_rows;
    p.blocked_apps = blocked;
    Ok(p)
}

// ---------------------------------------------------------------- items

fn refused(id: &str, name: &str, why: &str) -> ItemResult {
    ItemResult {
        id: id.to_string(),
        name: name.to_string(),
        ok: false,
        message: format!("Refused: {why}"),
    }
}

fn summarize_items(part: Part, items: Vec<ItemResult>, what: &str) -> PartResult {
    let ok = items.iter().filter(|i| i.ok).count() as u64;
    let bad = items.len() as u64 - ok;
    let status = if bad == 0 {
        PartStatus::Done
    } else if ok > 0 {
        PartStatus::Partial
    } else {
        PartStatus::Failed
    };
    let message = if bad == 0 {
        format!("{} {what}.", plural(ok, "item", "items"))
    } else {
        format!("{ok} of {} {what}; {bad} failed.", ok + bad)
    };
    let mut p = PartResult::new(part, status, message);
    p.items = items;
    p
}

/// The pre-fix state of startup items and apps: what was on offer when the user decided.
struct Snapshot {
    analysis: optimizer::Analysis,
}

fn snapshot(ctx: &Ctx, job: &Job) -> Result<Snapshot> {
    Ok(Snapshot {
        analysis: optimizer::analyze(ctx, job)?,
    })
}

fn startup_part(ctx: &Ctx, ids: &[String], snap: &Snapshot, job: &Job) -> PartResult {
    let mut items = Vec::new();
    for id in dedupe(ids) {
        if job.is_cancelled() {
            items.push(ItemResult {
                id: id.clone(),
                name: id,
                ok: false,
                message: "Not run: cancelled".into(),
            });
            continue;
        }
        let Some(e) = snap.analysis.entries.iter().find(|e| e.item.id == id) else {
            items.push(refused(&id, &id, "that startup item no longer exists"));
            continue;
        };
        let name = e.item.name.clone();
        if e.item.critical {
            items.push(refused(&id, &name, "it is needed by the system"));
            continue;
        }
        if !e.item.enabled {
            // Already what the user wants; nothing to change.
            items.push(ItemResult {
                id,
                name,
                ok: true,
                message: "Already switched off.".into(),
            });
            continue;
        }
        if !startup_candidate(e) {
            items.push(refused(
                &id,
                &name,
                "this kind of item is managed on the Startup page",
            ));
            continue;
        }
        match startup::apply_enabled(ctx, e, false, ToggleOpts::default()) {
            Ok(()) => items.push(ItemResult {
                id,
                name,
                ok: true,
                message: "Switched off.".into(),
            }),
            Err(err) => items.push(ItemResult {
                id,
                name,
                ok: false,
                message: err.message,
            }),
        }
    }
    summarize_items(Part::Startup, items, "switched off")
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn sleep_part(ctx: &Ctx, ids: &[String], snap: &Snapshot, job: &Job) -> Result<PartResult> {
    let allowed: Vec<(String, String, u64)> = background_apps(&snap.analysis);
    let mut items = Vec::new();
    let mut valid: Vec<String> = Vec::new();
    for id in dedupe(ids) {
        match allowed.iter().find(|a| a.0 == id) {
            Some(_) => valid.push(id),
            None => items.push(refused(
                &id,
                &id,
                "it is not a background app that can be put to sleep",
            )),
        }
    }
    if !valid.is_empty() {
        let v = dispatch(ctx, "optimizer.sleep", json!({ "appIds": valid }), job)?;
        for r in v["results"].as_array().cloned().unwrap_or_default() {
            let ok = r["ok"].as_bool().unwrap_or(false);
            let mut msg = if ok {
                "Put to sleep.".to_string()
            } else {
                let errs: Vec<String> = r["errors"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                if errs.is_empty() {
                    str_of(&r, "error")
                } else {
                    errs.join("; ")
                }
            };
            if let Some(note) = r["note"].as_str() {
                msg = format!("{msg} {note}");
            }
            items.push(ItemResult {
                id: str_of(&r, "appId"),
                name: str_of(&r, "name"),
                ok,
                message: msg,
            });
        }
    }
    Ok(summarize_items(Part::Sleep, items, "put to sleep"))
}

fn updates_part(ctx: &Ctx, ids: &[String], job: &Job) -> Result<PartResult> {
    let ignored: HashSet<String> = settings::load(ctx).ignored_updates.into_iter().collect();
    let mut items = Vec::new();
    let mut valid: Vec<String> = Vec::new();
    for id in dedupe(ids) {
        if ignored.contains(&id) {
            items.push(refused(&id, &id, "you chose to ignore this update"));
        } else {
            valid.push(id);
        }
    }
    if !valid.is_empty() {
        // The updater matches the ids against a fresh listing and refuses unknown ones.
        let v = dispatch(ctx, "software_updater.update", json!({ "ids": valid }), job)?;
        for r in v["results"].as_array().cloned().unwrap_or_default() {
            items.push(ItemResult {
                id: str_of(&r, "id"),
                name: {
                    let n = str_of(&r, "name");
                    if n.is_empty() {
                        str_of(&r, "id")
                    } else {
                        n
                    }
                },
                ok: r["ok"].as_bool().unwrap_or(false),
                message: str_of(&r, "message"),
            });
        }
    }
    Ok(summarize_items(Part::Updates, items, "updated"))
}

// ---------------------------------------------------------------- driver

fn run_part(
    ctx: &Ctx,
    req: &FixRequest,
    part: Part,
    snap: &mut Option<Snapshot>,
    job: &Job,
) -> Result<PartResult> {
    match part {
        Part::Privacy | Part::Space => {
            let s = settings::load(ctx);
            let (privacy, space) = analyze::enabled_rule_ids(ctx, &s)?;
            let ids = if part == Part::Privacy { privacy } else { space };
            clean_part(ctx, part, ids, req.close_apps, job)
        }
        Part::Startup | Part::Sleep => {
            if snap.is_none() {
                *snap = Some(snapshot(ctx, job)?);
            }
            let s = snap.as_ref().expect("snapshot was just taken");
            if part == Part::Startup {
                Ok(startup_part(ctx, &req.startup_ids, s, job))
            } else {
                sleep_part(ctx, &req.sleep_app_ids, s, job)
            }
        }
        Part::Updates => updates_part(ctx, &req.update_ids, job),
    }
}

/// Do what `req` lists, part by part (cancellable in between), then re-scan.
pub fn fix(ctx: &Ctx, req: &FixRequest, job: &Job) -> Result<FixReport> {
    let parts = req.parts();
    if parts.is_empty() {
        return Err(ApiError::invalid_params("nothing to fix: no part was selected"));
    }
    // One slice per part plus one for the final re-scan.
    let slices = parts.len() as f64 + 1.0;
    let mut results: Vec<PartResult> = Vec::with_capacity(parts.len());
    let mut snap: Option<Snapshot> = None;
    let mut cancelled = false;

    for (i, part) in parts.iter().copied().enumerate() {
        if cancelled || job.is_cancelled() {
            cancelled = true;
            results.push(PartResult::new(part, PartStatus::NotRun, "Not run: cancelled."));
            continue;
        }
        let base = i as f64 / slices;
        job.progress(
            ProgressEvent::new("health")
                .fraction(base)
                .counts(i as u64, parts.len() as u64)
                .message(label(part)),
        );
        let r = scoped(job, base, 1.0 / slices, label(part), |inner| {
            run_part(ctx, req, part, &mut snap, inner)
        });
        match r {
            Ok(p) => results.push(p),
            Err(e) if e.code == ErrorCode::Cancelled => {
                cancelled = true;
                results.push(PartResult::new(
                    part,
                    PartStatus::Partial,
                    "Cancelled part-way; some changes may have been made.",
                ));
            }
            Err(e) => results.push(PartResult::new(part, PartStatus::Failed, e.message)),
        }
        if job.is_cancelled() {
            cancelled = true;
        }
    }

    if !cancelled {
        job.progress(
            ProgressEvent::new("health")
                .fraction(parts.len() as f64 / slices)
                .message("Re-checking"),
        );
        let base = parts.len() as f64 / slices;
        let fresh = scoped(job, base, 1.0 / slices, "Re-checking", |inner| {
            analyze::analyze(ctx, inner)
        });
        match fresh {
            Ok(report) => {
                // The result is fresh news for the Home tab; failing to store it is not fatal.
                let _ = store::save(ctx, &report);
                return Ok(FixReport {
                    parts: results,
                    cancelled: false,
                    report: Some(report),
                });
            }
            Err(e) if e.code == ErrorCode::Cancelled => cancelled = true,
            Err(e) => return Err(e),
        }
    }
    // What was stored no longer describes the machine.
    store::clear(ctx);
    Ok(FixReport {
        parts: results,
        cancelled,
        report: None,
    })
}
