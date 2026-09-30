//! Performance optimizer: "sleep mode" for apps.
//!
//! An app is put to sleep by disabling the startup items that launch it and asking its
//! background processes to quit. Nothing is ever force-killed, and everything that was
//! changed is recorded in `<data>/optimizer.json` so that waking the app restores exactly
//! that and nothing else (an item that was already disabled before sleep stays disabled).
//!
//! Methods:
//! - `optimizer.analyze`: `{apps: [...], totals}` from the startup items and running processes.
//! - `optimizer.sleep { appIds }` / `optimizer.wake { appIds }`.
//! - `optimizer.enforce`: for sleeping apps, disable startup items that were re-enabled or
//!   re-created since (apps such as chat clients re-register their autostart entry when
//!   launched). Only startup items are touched: processes the user started are left alone.
//!   Meant to be called periodically by the background agent.
//!
//! Ids from the client only *select*: apps and startup items are looked up again on the server.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::api::Registry;
use crate::ctx::{Ctx, Os};
use crate::elevate::applescript_escape;
use crate::error::{ApiError, Result};
use crate::features::startup::{self, Entry, ToggleOpts};
use crate::fsutil::{atomic_write, now_rfc3339};
use crate::job::{Job, ProgressEvent};
use crate::procs::ProcDetail;

pub mod apps;

#[cfg(test)]
mod tests;

use apps::App;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "optimizer.analyze",
    "optimizer.sleep",
    "optimizer.wake",
    "optimizer.enforce",
];

pub fn register(r: &mut Registry) {
    r.add("optimizer.analyze", analyze_handler);
    r.add("optimizer.sleep", sleep_handler);
    r.add("optimizer.wake", wake_handler);
    r.add("optimizer.enforce", enforce_handler);
}

/// How long a process gets to quit after being asked to (milliseconds). Never followed by
/// a kill.
static QUIT_WAIT_MS: AtomicU64 = AtomicU64::new(5000);

pub fn quit_wait() -> Duration {
    Duration::from_millis(QUIT_WAIT_MS.load(Ordering::Relaxed))
}

/// Shorten the wait (tests only).
#[cfg(test)]
pub fn set_quit_wait_ms(ms: u64) {
    QUIT_WAIT_MS.store(ms, Ordering::Relaxed);
}

// ---------------------------------------------------------------- state

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub sleeping: BTreeMap<String, Sleeping>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sleeping {
    pub name: String,
    pub slept_at: String,
    /// Items sleep (or enforce) disabled: exactly these are enabled again on wake.
    pub disabled: Vec<Disabled>,
    /// Items that were already disabled when the app went to sleep: left disabled on wake.
    #[serde(default)]
    pub already_disabled: Vec<String>,
    #[serde(default)]
    pub stopped: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Disabled {
    pub id: String,
    pub name: String,
    /// Found re-enabled / newly created by `enforce` rather than disabled by `sleep`.
    #[serde(default)]
    pub by_enforce: bool,
}

static STATE_LOCK: Mutex<()> = Mutex::new(());

fn state_path(ctx: &Ctx) -> std::path::PathBuf {
    ctx.env.data_dir.join("optimizer.json")
}

pub fn load_state(ctx: &Ctx) -> Result<State> {
    match std::fs::read_to_string(state_path(ctx)) {
        Ok(t) if t.trim().is_empty() => Ok(State::default()),
        Ok(t) => serde_json::from_str(&t).map_err(|e| {
            ApiError::io(format!(
                "optimizer.json is damaged ({e}); delete {} to reset sleep mode",
                state_path(ctx).display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(e.into()),
    }
}

fn save_state(ctx: &Ctx, s: &State) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(s)?;
    atomic_write(&state_path(ctx), &bytes)
        .map_err(|e| ApiError::io(format!("could not save optimizer.json: {e}")))
}

// ---------------------------------------------------------------- analysis

pub struct Analysis {
    pub entries: Vec<Entry>,
    pub apps: Vec<App>,
    pub state: State,
    pub state_error: Option<String>,
}

pub fn analyze(ctx: &Ctx, job: &Job) -> Result<Analysis> {
    job.progress(ProgressEvent::new("analyze").message("Reading running programs"));
    let procs = ctx.procs.details();
    job.check_cancelled()?;
    let entries = startup::collect_with_procs(ctx, job, &procs)?;
    job.check_cancelled()?;
    let apps = apps::build_apps(ctx, &entries, &procs, std::process::id());
    let (state, state_error) = match load_state(ctx) {
        Ok(s) => (s, None),
        Err(e) => (State::default(), Some(e.message)),
    };
    Ok(Analysis {
        entries,
        apps,
        state,
        state_error,
    })
}

fn app_json(a: &App, sleeping: bool) -> Value {
    let mem: u64 = a.procs.iter().map(|p| p.memory_bytes).sum();
    let cpu: f32 = a.procs.iter().map(|p| p.cpu_percent).sum();
    let mut procs: Vec<&ProcDetail> = a.procs.iter().collect();
    procs.sort_by_key(|p| std::cmp::Reverse(p.memory_bytes));
    json!({
        "appId": a.id,
        "name": a.name,
        "icon": a.icon,
        "processes": procs.iter().map(|p| json!({
            "pid": p.pid,
            "name": p.name,
            "memoryBytes": p.memory_bytes,
            "cpuPercent": (p.cpu_percent * 10.0).round() / 10.0,
        })).collect::<Vec<_>>(),
        "startupIds": a.startup_ids,
        "serviceIds": a.service_ids,
        "backgroundMemoryBytes": mem,
        "cpuPercent": (cpu * 10.0).round() / 10.0,
        "sleeping": sleeping,
        "protected": a.protected,
    })
}

fn analyze_handler(ctx: &Ctx, _params: Value, job: &Job) -> Result<Value> {
    let a = analyze(ctx, job)?;
    let mut list: Vec<Value> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for app in &a.apps {
        seen.insert(&app.id);
        list.push(app_json(app, a.state.sleeping.contains_key(&app.id)));
    }
    // Sleeping apps that currently have nothing to show (no process, entries removed).
    for (id, s) in &a.state.sleeping {
        if !seen.contains(id.as_str()) {
            list.push(json!({
                "appId": id,
                "name": s.name,
                "icon": null,
                "processes": [],
                "startupIds": s.disabled.iter().map(|d| d.id.clone()).collect::<Vec<_>>(),
                "serviceIds": [],
                "backgroundMemoryBytes": 0,
                "cpuPercent": 0.0,
                "sleeping": true,
                "protected": false,
            }));
        }
    }
    let running = a.apps.iter().filter(|x| !x.procs.is_empty()).count();
    let mem: u64 = a
        .apps
        .iter()
        .filter(|x| !a.state.sleeping.contains_key(&x.id))
        .flat_map(|x| x.procs.iter())
        .map(|p| p.memory_bytes)
        .sum();
    let items = a
        .apps
        .iter()
        .flat_map(|x| x.startup_ids.iter().chain(x.service_ids.iter()))
        .filter(|id| {
            a.entries
                .iter()
                .any(|e| &e.item.id == *id && e.item.enabled)
        })
        .count();
    list.sort_by(|x, y| {
        let key = |v: &Value| {
            (
                v["sleeping"].as_bool().unwrap_or(false),
                std::cmp::Reverse(v["backgroundMemoryBytes"].as_u64().unwrap_or(0)),
                v["name"].as_str().unwrap_or("").to_lowercase(),
            )
        };
        key(x).cmp(&key(y))
    });
    Ok(json!({
        "apps": list,
        "totals": {
            "apps": list.len(),
            "runningApps": running,
            "backgroundMemoryBytes": mem,
            "sleepingApps": a.state.sleeping.len(),
            "startupItems": items,
        },
        "stateError": a.state_error,
    }))
}

// ---------------------------------------------------------------- stopping processes

/// Ask `app`'s processes to quit and wait up to [`QUIT_WAIT`]. Returns
/// `(gone, still_running, delivered)`; nothing is killed.
fn stop_processes(ctx: &Ctx, app: &App, job: &Job) -> (usize, usize, usize) {
    let pids: Vec<(u32, String)> = app.procs.iter().map(|p| (p.pid, p.name.clone())).collect();
    if pids.is_empty() {
        return (0, 0, 0);
    }
    // Only signal a pid that still belongs to the same process (pids get reused).
    let still_same = |ctx: &Ctx| -> Vec<u32> {
        let now = ctx.procs.list();
        pids.iter()
            .filter(|(pid, name)| now.iter().any(|p| p.pid == *pid && p.name == *name))
            .map(|(pid, _)| *pid)
            .collect()
    };
    let mut delivered = 0usize;
    let start = Instant::now();
    match ctx.env.os {
        Os::Windows => {
            for pid in still_same(ctx) {
                let ok = ctx
                    .runner
                    .run("taskkill", &["/PID", &pid.to_string()])
                    .is_ok_and(|o| o.success());
                delivered += ok as usize;
            }
        }
        Os::MacOs => {
            let script = format!(
                "tell application \"{}\" to quit",
                applescript_escape(&app.name)
            );
            if ctx
                .runner
                .run("osascript", &["-e", &script])
                .is_ok_and(|o| o.success())
            {
                delivered += 1;
            }
            // Give the app a moment to quit by itself before asking the OS to.
            wait_gone(ctx, &still_same(ctx), Duration::from_secs(2), job);
            for pid in still_same(ctx) {
                if ctx.procs.request_exit(pid) {
                    delivered += 1;
                }
            }
        }
        Os::Linux => {
            for pid in still_same(ctx) {
                if ctx.procs.request_exit(pid) {
                    delivered += 1;
                }
            }
        }
    }
    if delivered > 0 {
        let left = quit_wait().saturating_sub(start.elapsed());
        wait_gone(ctx, &still_same(ctx), left, job);
    }
    let remaining = still_same(ctx).len();
    (pids.len() - remaining, remaining, delivered)
}

fn wait_gone(ctx: &Ctx, pids: &[u32], timeout: Duration, job: &Job) {
    if pids.is_empty() {
        return;
    }
    let deadline = Instant::now() + timeout;
    loop {
        let now = ctx.procs.list();
        if !pids.iter().any(|pid| now.iter().any(|p| p.pid == *pid)) {
            return;
        }
        if Instant::now() >= deadline || job.is_cancelled() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100).min(timeout));
    }
}

// ---------------------------------------------------------------- params

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppIds {
    app_ids: Vec<String>,
}

fn parse_ids(params: Value) -> Result<Vec<String>> {
    let p: AppIds = serde_json::from_value(params)?;
    if p.app_ids.is_empty() {
        return Err(ApiError::invalid_params("`appIds` is empty"));
    }
    if p.app_ids.len() > 500 || p.app_ids.iter().any(|i| i.is_empty() || i.len() > 300) {
        return Err(ApiError::invalid_params("invalid `appIds`"));
    }
    let mut seen = HashSet::new();
    Ok(p.app_ids
        .into_iter()
        .filter(|i| seen.insert(i.clone()))
        .collect())
}

fn entry_by_id<'a>(entries: &'a [Entry], id: &str) -> Option<&'a Entry> {
    entries.iter().find(|e| e.item.id == id)
}

// ---------------------------------------------------------------- sleep

fn sleep_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let ids = parse_ids(params)?;
    let _guard = STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let a = analyze(ctx, job)?;
    if let Some(e) = &a.state_error {
        return Err(ApiError::io(e.clone()));
    }
    let mut state = a.state.clone();
    let mut results = Vec::new();
    for (n, id) in ids.iter().enumerate() {
        job.check_cancelled()?;
        job.progress(
            ProgressEvent::new("sleep")
                .fraction(n as f64 / ids.len() as f64)
                .counts(n as u64, ids.len() as u64)
                .message(format!("Putting {id} to sleep")),
        );
        let Some(app) = a.apps.iter().find(|x| &x.id == id) else {
            results.push(json!({
                "appId": id, "name": id, "ok": false,
                "error": "That app is no longer running or set to start automatically.",
            }));
            continue;
        };
        if app.protected {
            results.push(json!({
                "appId": id, "name": app.name, "ok": false,
                "error": "This app is protected and cannot be put to sleep.",
            }));
            continue;
        }
        let rec = state
            .sleeping
            .entry(id.clone())
            .or_insert_with(|| Sleeping {
                name: app.name.clone(),
                slept_at: now_rfc3339(),
                disabled: Vec::new(),
                already_disabled: Vec::new(),
                stopped: 0,
            });
        let mut errors: Vec<String> = Vec::new();
        let mut newly: Vec<String> = Vec::new();
        for item_id in app.startup_ids.iter().chain(app.service_ids.iter()) {
            job.check_cancelled()?;
            let Some(e) = entry_by_id(&a.entries, item_id) else {
                continue;
            };
            if rec.disabled.iter().any(|d| &d.id == item_id) {
                // Ours already; make sure it is still off.
                if e.item.enabled {
                    if let Err(err) = startup::apply_enabled(ctx, e, false, ToggleOpts::default()) {
                        errors.push(format!("{}: {}", e.item.name, err.message));
                    }
                }
                continue;
            }
            if !e.item.enabled {
                if !rec.already_disabled.contains(item_id) {
                    rec.already_disabled.push(item_id.clone());
                }
                continue;
            }
            match startup::apply_enabled(ctx, e, false, ToggleOpts::default()) {
                Ok(()) => {
                    rec.disabled.push(Disabled {
                        id: item_id.clone(),
                        name: e.item.name.clone(),
                        by_enforce: false,
                    });
                    newly.push(item_id.clone());
                }
                Err(err) => errors.push(format!("{}: {}", e.item.name, err.message)),
            }
        }
        // Persist what was changed before the slower step of stopping processes.
        let nothing_changed = rec.disabled.is_empty() && app.procs.is_empty();
        if nothing_changed {
            state.sleeping.remove(id);
        }
        save_state(ctx, &state)?;

        let (gone, remaining, delivered) = stop_processes(ctx, app, job);
        if let Some(rec) = state.sleeping.get_mut(id) {
            rec.stopped += gone as u32;
        }
        save_state(ctx, &state)?;
        let mut note: Option<String> = None;
        if remaining > 0 {
            note = Some(if delivered == 0 {
                "The app could not be asked to quit; its startup entries are switched off.".into()
            } else {
                format!(
                    "{remaining} process{} did not quit within {} seconds. They are left running; close the app yourself.",
                    if remaining == 1 { "" } else { "es" },
                    quit_wait().as_secs()
                )
            });
        }
        results.push(json!({
            "appId": id,
            "name": app.name,
            "ok": errors.is_empty(),
            "disabledItems": newly,
            "stoppedProcesses": gone,
            "stillRunning": remaining,
            "note": note,
            "errors": errors,
        }));
    }
    Ok(json!({ "results": results }))
}

// ---------------------------------------------------------------- wake

fn wake_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let ids = parse_ids(params)?;
    let _guard = STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut state = load_state(ctx)?;
    let entries = startup::collect(ctx, job)?;
    let mut results = Vec::new();
    for (n, id) in ids.iter().enumerate() {
        job.check_cancelled()?;
        job.progress(
            ProgressEvent::new("wake")
                .fraction(n as f64 / ids.len() as f64)
                .counts(n as u64, ids.len() as u64)
                .message(format!("Waking {id}")),
        );
        let Some(rec) = state.sleeping.get(id).cloned() else {
            results.push(json!({
                "appId": id, "name": id, "ok": false, "error": "This app is not asleep.",
            }));
            continue;
        };
        let mut restored: Vec<String> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        let mut keep: Vec<Disabled> = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        for d in &rec.disabled {
            match entry_by_id(&entries, &d.id) {
                None => missing.push(d.name.clone()),
                Some(e) if e.item.enabled => restored.push(d.id.clone()),
                Some(e) => match startup::apply_enabled(ctx, e, true, ToggleOpts::default()) {
                    Ok(()) => restored.push(d.id.clone()),
                    Err(err) => {
                        errors.push(format!("{}: {}", d.name, err.message));
                        keep.push(d.clone());
                    }
                },
            }
        }
        if keep.is_empty() {
            state.sleeping.remove(id);
        } else if let Some(r) = state.sleeping.get_mut(id) {
            r.disabled = keep;
        }
        save_state(ctx, &state)?;
        results.push(json!({
            "appId": id,
            "name": rec.name,
            "ok": errors.is_empty(),
            "restoredItems": restored,
            "missingItems": missing,
            "errors": errors,
        }));
    }
    Ok(json!({ "results": results }))
}

// ---------------------------------------------------------------- enforce

fn enforce_handler(ctx: &Ctx, _params: Value, job: &Job) -> Result<Value> {
    let _guard = STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut state = load_state(ctx)?;
    if state.sleeping.is_empty() {
        return Ok(json!({ "changed": [], "errors": [] }));
    }
    // Deliberately no process snapshot: enforce never looks at, or touches, processes.
    let entries = startup::collect_with_procs(ctx, job, &[])?;
    let mut changed: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let ids: Vec<String> = state.sleeping.keys().cloned().collect();
    for app_id in ids {
        job.check_cancelled()?;
        let Some(rec) = state.sleeping.get(&app_id).cloned() else {
            continue;
        };
        let mut new_disabled: Vec<Disabled> = Vec::new();
        for e in &entries {
            if !apps::is_app_item(e) || !e.item.enabled {
                continue;
            }
            let recorded = rec.disabled.iter().any(|d| d.id == e.item.id);
            if !recorded && apps::entry_app_key(e) != app_id {
                continue;
            }
            match startup::apply_enabled(ctx, e, false, ToggleOpts::default()) {
                Ok(()) => {
                    changed.push(json!({
                        "appId": app_id,
                        "itemId": e.item.id,
                        "name": e.item.name,
                        "action": "disabled",
                        "reason": if recorded { "re-enabled" } else { "created" },
                    }));
                    if !recorded {
                        new_disabled.push(Disabled {
                            id: e.item.id.clone(),
                            name: e.item.name.clone(),
                            by_enforce: true,
                        });
                    }
                }
                Err(err) => errors.push(format!("{}: {}", e.item.name, err.message)),
            }
        }
        if !new_disabled.is_empty() {
            if let Some(r) = state.sleeping.get_mut(&app_id) {
                // An item that was on the "already disabled" list and came back is now ours.
                r.already_disabled
                    .retain(|id| !new_disabled.iter().any(|d| &d.id == id));
                r.disabled.extend(new_disabled);
            }
            save_state(ctx, &state)?;
        }
    }
    Ok(json!({ "changed": changed, "errors": errors }))
}
