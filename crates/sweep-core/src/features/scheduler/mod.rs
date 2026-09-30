//! Scheduler: run cleans on a schedule.
//!
//! Schedules live in `<data>/schedules.json` and are mirrored into the operating system's own
//! scheduler (see [`sync`]) so they run even when ClearSweep is closed. The OS job runs
//! `<job_exe> clean --auto --source scheduled --schedule <id>`, which records `lastRun` /
//! `lastResult` through [`run_schedule`].
//!
//! Methods:
//! - `scheduler.list`: every schedule (`{id, name, enabled, frequency, time, weekdays,
//!   dayOfMonth, action, createdAt, lastRun?, lastResult?}`).
//! - `scheduler.add {name, frequency, time?, weekdays?, dayOfMonth?, enabled?, action?}`.
//! - `scheduler.update {id, ...any of the above}`.
//! - `scheduler.remove {id}`: `{removed, warnings}`; the OS job is removed best effort.
//! - `scheduler.set_enabled {id, enabled}`.
//! - `scheduler.run_now {id}`: run the schedule's clean immediately (also when disabled) and
//!   record the result: `{schedule, report}`.
//! - `scheduler.backend`: `{kind, available, detail}` for the OS mechanism in use.
//!
//! Add / update / set_enabled change the OS first and save only when that worked, so the
//! file never claims a job the OS refused. `remove` deletes the schedule even when the OS
//! cleanup partly fails (the leftovers are reported as warnings).

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::Registry;
use crate::clock::{ms_from_rfc3339, Clock};
use crate::ctx::Ctx;
use crate::error::{ApiError, ErrorCode, Result};
use crate::features::cleaner::{self, CleanOptions, CleanReport, Source};
use crate::features::settings;
use crate::fsutil::{atomic_write, now_rfc3339, random_id};
use crate::job::Job;

pub mod gen;
pub mod model;
pub mod sync;

#[cfg(test)]
mod tests;

pub use model::{Action, ActionKind, Frequency, LastResult, NewSchedule, Schedule, UpdateSchedule};

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "scheduler.list",
    "scheduler.add",
    "scheduler.update",
    "scheduler.remove",
    "scheduler.set_enabled",
    "scheduler.run_now",
    "scheduler.backend",
];

pub fn register(r: &mut Registry) {
    r.add("scheduler.list", list_handler);
    r.add("scheduler.add", add_handler);
    r.add("scheduler.update", update_handler);
    r.add("scheduler.remove", remove_handler);
    r.add("scheduler.set_enabled", set_enabled_handler);
    r.add("scheduler.run_now", run_now_handler);
    r.add("scheduler.backend", backend_handler);
}

/// Runs missed by the OS scheduler are only caught up when they are at least this late.
const CATCH_UP_GRACE_MIN: i64 = 10;

// ---------------------------------------------------------------- store

static LOCK: Mutex<()> = Mutex::new(());

fn store_path(ctx: &Ctx) -> PathBuf {
    ctx.env.data_dir.join("schedules.json")
}

/// Serialises read-modify-write cycles between threads and between processes (the desktop
/// app, the agent and OS-launched jobs all write this file).
struct StoreGuard {
    _mutex: std::sync::MutexGuard<'static, ()>,
    file: fs::File,
}

impl Drop for StoreGuard {
    fn drop(&mut self) {
        let _ = fs4::FileExt::unlock(&self.file);
    }
}

fn lock_store(ctx: &Ctx) -> Result<StoreGuard> {
    let mutex = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    fs::create_dir_all(&ctx.env.data_dir)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(ctx.env.data_dir.join("schedules.lock"))?;
    fs4::FileExt::lock(&file)?;
    Ok(StoreGuard {
        _mutex: mutex,
        file,
    })
}

/// All schedules. A damaged file is an error (never silently replaced).
pub fn load(ctx: &Ctx) -> Result<Vec<Schedule>> {
    let bytes = match fs::read(store_path(ctx)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    let all: Vec<Schedule> = serde_json::from_slice(&bytes).map_err(|e| {
        ApiError::io(format!(
            "schedules.json is damaged ({e}); fix or delete {}",
            store_path(ctx).display()
        ))
    })?;
    if let Some(bad) = all.iter().find(|s| !model::valid_id(&s.id)) {
        return Err(ApiError::io(format!(
            "schedules.json has an entry with an invalid id `{}`",
            bad.id
        )));
    }
    Ok(all)
}

fn save(ctx: &Ctx, all: &[Schedule]) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(all)?;
    atomic_write(&store_path(ctx), &bytes)
        .map_err(|e| ApiError::io(format!("could not save schedules.json: {e}")))
}

fn known_rule_ids(ctx: &Ctx) -> Vec<String> {
    let s = settings::load(ctx);
    cleaner::available_rules(ctx, &s)
        .into_iter()
        .map(|r| r.id)
        .collect()
}

fn find(all: &[Schedule], id: &str) -> Result<usize> {
    all.iter()
        .position(|s| s.id == id)
        .ok_or_else(|| ApiError::not_found(format!("no schedule with id `{id}`")))
}

// ---------------------------------------------------------------- operations

pub fn add(ctx: &Ctx, new: NewSchedule) -> Result<Schedule> {
    add_with_exe(ctx, new, &crate::exe::job_exe())
}

pub fn add_with_exe(ctx: &Ctx, new: NewSchedule, exe: &std::path::Path) -> Result<Schedule> {
    let _g = lock_store(ctx)?;
    let mut all = load(ctx)?;
    if all.len() >= model::MAX_SCHEDULES {
        return Err(ApiError::invalid_params(format!(
            "at most {} schedules are supported",
            model::MAX_SCHEDULES
        )));
    }
    let mut s = Schedule {
        id: random_id(),
        name: new.name,
        enabled: new.enabled,
        frequency: new.frequency,
        time: new.time,
        weekdays: new.weekdays,
        day_of_month: new.day_of_month,
        action: new.action,
        created_at: now_rfc3339(),
        last_run: None,
        last_result: None,
    };
    s.validate(&all, &known_rule_ids(ctx))?;
    sync::sync(ctx, exe, &s).inspect_err(|_| {
        // A half-installed job must not outlive the failed add.
        let _ = sync::remove(ctx, &s.id);
    })?;
    all.push(s.clone());
    save(ctx, &all)?;
    Ok(s)
}

pub fn update(ctx: &Ctx, u: UpdateSchedule) -> Result<Schedule> {
    update_with_exe(ctx, u, &crate::exe::job_exe())
}

pub fn update_with_exe(ctx: &Ctx, u: UpdateSchedule, exe: &std::path::Path) -> Result<Schedule> {
    let _g = lock_store(ctx)?;
    let mut all = load(ctx)?;
    let i = find(&all, &u.id)?;
    let old = all[i].clone();
    let mut s = old.clone();
    if let Some(v) = u.name {
        s.name = v;
    }
    if let Some(v) = u.enabled {
        s.enabled = v;
    }
    if let Some(v) = u.frequency {
        s.frequency = v;
    }
    if let Some(v) = u.time {
        s.time = v;
    }
    if let Some(v) = u.weekdays {
        s.weekdays = v;
    }
    if let Some(v) = u.day_of_month {
        s.day_of_month = v;
    }
    if let Some(v) = u.action {
        s.action = v;
    }
    let others: Vec<Schedule> = all.iter().filter(|x| x.id != s.id).cloned().collect();
    s.validate(&others, &known_rule_ids(ctx))?;
    if s == old {
        return Ok(s);
    }
    // Moving from `on_login` to a timed schedule (or back) would leave the other kind's
    // artifact behind: clear the old one first.
    if (old.frequency == Frequency::OnLogin) != (s.frequency == Frequency::OnLogin) {
        let _ = sync::remove(ctx, &old.id);
    }
    if let Err(e) = sync::sync(ctx, exe, &s) {
        // Put the OS back the way the saved schedule says.
        let _ = sync::sync(ctx, exe, &old);
        return Err(e);
    }
    all[i] = s.clone();
    save(ctx, &all)?;
    Ok(s)
}

pub fn remove(ctx: &Ctx, id: &str) -> Result<Vec<String>> {
    let _g = lock_store(ctx)?;
    let mut all = load(ctx)?;
    let i = find(&all, id)?;
    let warnings = sync::remove(ctx, id);
    all.remove(i);
    save(ctx, &all)?;
    Ok(warnings)
}

// ---------------------------------------------------------------- running

/// Outcome of [`run_schedule`].
pub struct RunOutcome {
    pub schedule: Schedule,
    pub report: Option<CleanReport>,
    pub error: Option<ApiError>,
}

fn record(ctx: &Ctx, id: &str, result: LastResult) -> Result<Schedule> {
    let _g = lock_store(ctx)?;
    let mut all = load(ctx)?;
    let i = find(&all, id)?;
    all[i].last_run = Some(now_rfc3339());
    all[i].last_result = Some(result);
    save(ctx, &all)?;
    Ok(all[i].clone())
}

/// Run schedule `id`'s clean (source `scheduled`) and record `lastRun` / `lastResult`.
///
/// With `require_enabled` (OS-launched jobs) a disabled schedule is refused with
/// `InvalidParams`, so a job the OS could not remove in time cannot run. A failing clean is
/// recorded as a failed result and returned in [`RunOutcome::error`].
pub fn run_schedule(ctx: &Ctx, id: &str, job: &Job, require_enabled: bool) -> Result<RunOutcome> {
    let s = {
        let _g = lock_store(ctx)?;
        let all = load(ctx)?;
        all[find(&all, id)?].clone()
    };
    if require_enabled && !s.enabled {
        return Err(ApiError::invalid_params(format!(
            "schedule `{}` is disabled",
            s.name
        )));
    }
    let settings = settings::load(ctx);
    // Nobody can answer "ask" in a scheduled run, so it behaves like "skip".
    let opts = CleanOptions::from_settings(&settings, true);
    let run = cleaner::run_clean(ctx, s.action.rules.clone(), &opts, Source::Scheduled, job);
    match run {
        Ok(report) => {
            let skipped = report
                .results
                .iter()
                .filter(|r| r.skipped.is_some())
                .count();
            let message = if report.cancelled {
                Some("cancelled".to_string())
            } else if skipped > 0 {
                Some(format!("{skipped} rule(s) skipped (app running or in use)"))
            } else {
                None
            };
            let schedule = record(
                ctx,
                id,
                LastResult {
                    ok: !report.cancelled,
                    total_bytes: report.total_bytes,
                    total_files: report.total_files,
                    message,
                },
            )?;
            Ok(RunOutcome {
                schedule,
                report: Some(report),
                error: None,
            })
        }
        Err(e) => {
            let schedule = record(
                ctx,
                id,
                LastResult {
                    ok: false,
                    total_bytes: 0,
                    total_files: 0,
                    message: Some(e.message.clone()),
                },
            )?;
            Ok(RunOutcome {
                schedule,
                report: None,
                error: Some(e),
            })
        }
    }
}

/// Run the enabled schedules whose most recent due time was missed by the OS scheduler
/// (machine off, cron down, ...). A schedule counts as missed when it was due at least ten
/// minutes ago and has neither run (`lastRun`) nor been created since. Returns
/// `(id, Ok(bytes removed) | Err(message))` for each one that was run.
///
/// The elapsed time is measured on the local calendar, so around a daylight-saving change the
/// comparison can be an hour off: at worst one extra or one late clean.
pub fn catch_up(
    ctx: &Ctx,
    clock: &dyn Clock,
    job: &Job,
) -> Vec<(String, std::result::Result<u64, String>)> {
    let (Some(local), Ok(all)) = (clock.local(), load(ctx)) else {
        return Vec::new();
    };
    let now_ms = clock.now_ms();
    let mut ran = Vec::new();
    for s in all.iter().filter(|s| s.enabled) {
        let Some(due) = model::previous_due_minutes(s, &local) else {
            continue;
        };
        let late_min = local.local_minutes() - due;
        if late_min < CATCH_UP_GRACE_MIN {
            continue;
        }
        let due_ms = now_ms.saturating_sub(late_min as u64 * 60_000);
        let baseline = [s.last_run.as_deref(), Some(s.created_at.as_str())]
            .into_iter()
            .flatten()
            .filter_map(ms_from_rfc3339)
            .max()
            .unwrap_or(0);
        if due_ms <= baseline {
            continue;
        }
        if job.is_cancelled() {
            break;
        }
        match run_schedule(ctx, &s.id, job, true) {
            Ok(o) => match o.error {
                None => ran.push((s.id.clone(), Ok(o.report.map_or(0, |r| r.total_bytes)))),
                Some(e) => ran.push((s.id.clone(), Err(e.message))),
            },
            Err(e) if e.code == ErrorCode::NotFound => {}
            Err(e) => ran.push((s.id.clone(), Err(e.message))),
        }
    }
    ran
}

// ---------------------------------------------------------------- handlers

fn params<T: for<'de> Deserialize<'de>>(v: Value) -> Result<T> {
    Ok(serde_json::from_value(v)?)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnabledParams {
    id: String,
    enabled: bool,
}

fn list_handler(ctx: &Ctx, _p: Value, _job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(load(ctx)?)?)
}

fn add_handler(ctx: &Ctx, p: Value, _job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(add(ctx, params(p)?)?)?)
}

fn update_handler(ctx: &Ctx, p: Value, _job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(update(ctx, params(p)?)?)?)
}

fn remove_handler(ctx: &Ctx, p: Value, _job: &Job) -> Result<Value> {
    let p: IdParams = params(p)?;
    let warnings = remove(ctx, &p.id)?;
    Ok(json!({ "removed": true, "warnings": warnings }))
}

fn set_enabled_handler(ctx: &Ctx, p: Value, _job: &Job) -> Result<Value> {
    let p: EnabledParams = params(p)?;
    let s = update(
        ctx,
        UpdateSchedule {
            id: p.id,
            enabled: Some(p.enabled),
            ..Default::default()
        },
    )?;
    Ok(serde_json::to_value(s)?)
}

fn run_now_handler(ctx: &Ctx, p: Value, job: &Job) -> Result<Value> {
    let p: IdParams = params(p)?;
    let o = run_schedule(ctx, &p.id, job, false)?;
    if let Some(e) = o.error {
        return Err(e);
    }
    Ok(json!({ "schedule": o.schedule, "report": o.report }))
}

fn backend_handler(ctx: &Ctx, _p: Value, _job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(sync::detect(ctx))?)
}
