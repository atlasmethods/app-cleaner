//! The background agent: a small state machine that watches junk size, browser windows,
//! sleeping apps and scheduled cleans. It owns no thread and never sleeps: a driver (the
//! CLI's `agent` command, or the desktop app's background thread) calls [`Agent::tick`]
//! about once a second and the agent decides what is due.
//!
//! Tasks (all read the saved settings each time they run, so changes apply without a restart):
//! - every `smart.checkIntervalMinutes`: measure the junk the enabled rules would remove and,
//!   past the threshold, notify (rate limited) and/or auto-clean;
//! - every 10 s: watch the browsers listed in `smart.cleanOnBrowserClose`; when one has been
//!   closed for two consecutive polls, clean that browser's enabled rules;
//! - every `smart.enforceSleepMinutes`: `optimizer.enforce` (keeps sleeping apps asleep);
//! - every minute: catch up scheduled cleans the OS scheduler missed.
//!
//! Only one agent runs per user: [`AgentLock`] is an exclusive lock on `<data>/agent.lock`,
//! released by the OS when the process exits, however it exits. Progress is published in
//! `<data>/agent-status.json`.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::clock::{rfc3339_from_ms, Clock};
use crate::ctx::Ctx;
use crate::error::Result;
use crate::features::cleaner::{self, apps, rules::Category, CleanOptions, Source};
use crate::features::settings::{self, Settings};
use crate::features::smart_cleaning;
use crate::fsutil::atomic_write;
use crate::job::{CancelToken, Job};
use crate::pkgutil::human_bytes;
use crate::procs::ProcInfo;

pub const LOCK_FILE: &str = "agent.lock";
pub const STATUS_FILE: &str = "agent-status.json";
/// How often browsers are polled.
pub const POLL_MS: u64 = 10_000;
/// A browser must stay closed for this many consecutive polls before it is cleaned.
pub const CLOSED_POLLS: u8 = 2;
/// Junk notifications are limited to one per this long...
pub const NOTIFY_GAP_MS: u64 = 12 * 60 * 60 * 1000;
/// ...unless the junk has grown by more than this factor since the last one.
pub const NOTIFY_GROWTH: f64 = 1.5;
const SCHEDULE_POLL_MS: u64 = 60_000;

/// Sends a desktop notification. Implementations must never panic; failures are theirs to log.
pub trait Notifier: Send + Sync {
    fn notify(&self, title: &str, body: &str);
}

/// Drops every notification.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullNotifier;

impl Notifier for NullNotifier {
    fn notify(&self, _title: &str, _body: &str) {}
}

// ---------------------------------------------------------------- lock + status

/// Exclusive single-instance lock. Dropping it (or exiting) releases it.
#[derive(Debug)]
pub struct AgentLock {
    _file: File,
}

fn lock_file(ctx: &Ctx) -> Result<File> {
    fs::create_dir_all(&ctx.env.data_dir)?;
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(ctx.env.data_dir.join(LOCK_FILE))?)
}

impl AgentLock {
    /// `Ok(None)` when another agent holds the lock.
    pub fn try_acquire(ctx: &Ctx) -> Result<Option<AgentLock>> {
        let file = lock_file(ctx)?;
        match fs4::FileExt::try_lock(&file) {
            Ok(()) => Ok(Some(AgentLock { _file: file })),
            Err(fs4::TryLockError::WouldBlock) => Ok(None),
            Err(fs4::TryLockError::Error(e)) => Err(e.into()),
        }
    }
}

/// Is an agent (in this or another process) holding the lock right now?
pub fn agent_running(ctx: &Ctx) -> bool {
    matches!(AgentLock::try_acquire(ctx), Ok(None))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// RFC 3339: when this agent started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// RFC 3339: the last junk check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_check: Option<String>,
    /// Junk found by the last check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_junk_bytes: Option<u64>,
    /// What the agent last did, in words ("Cleaned 1.2 GB").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_action_at: Option<String>,
}

fn status_path(ctx: &Ctx) -> std::path::PathBuf {
    ctx.env.data_dir.join(STATUS_FILE)
}

fn write_status(ctx: &Ctx, s: &AgentStatus) {
    if let Ok(bytes) = serde_json::to_vec_pretty(s) {
        let _ = atomic_write(&status_path(ctx), &bytes);
    }
}

/// The published status. `running` is verified against the lock, so a crashed agent's
/// leftover file never claims to be running.
pub fn read_status(ctx: &Ctx) -> AgentStatus {
    let mut s: AgentStatus = fs::read(status_path(ctx))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    s.running = agent_running(ctx);
    if !s.running {
        s.pid = None;
    }
    s
}

// ---------------------------------------------------------------- state machine

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    JunkChecked { bytes: u64, over_threshold: bool },
    Notified { title: String, body: String },
    AutoCleaned { bytes: u64 },
    BrowserCleaned { group: String, bytes: u64 },
    Enforced { changed: usize },
    ScheduleCaughtUp { id: String },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Wait this long after start before the first junk check / enforce / schedule pass
    /// (browser polling starts at once so the first observation is a baseline).
    pub startup_delay_ms: u64,
    pub poll_ms: u64,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            startup_delay_ms: 30_000,
            poll_ms: POLL_MS,
        }
    }
}

#[derive(Debug, Default)]
struct Watch {
    was_running: bool,
    closed_polls: u8,
}

pub struct Agent {
    ctx: Ctx,
    clock: Arc<dyn Clock>,
    notifier: Arc<dyn Notifier>,
    cfg: AgentConfig,
    cancel: CancelToken,
    next_poll: u64,
    next_check: u64,
    next_enforce: u64,
    next_schedule: u64,
    watch: HashMap<String, Watch>,
    last_notified_at: Option<u64>,
    last_notified_bytes: u64,
    status: AgentStatus,
    dirty: bool,
}

impl Agent {
    pub fn new(ctx: Ctx, clock: Arc<dyn Clock>, notifier: Arc<dyn Notifier>) -> Self {
        Self::with_config(ctx, clock, notifier, AgentConfig::default())
    }

    pub fn with_config(
        ctx: Ctx,
        clock: Arc<dyn Clock>,
        notifier: Arc<dyn Notifier>,
        cfg: AgentConfig,
    ) -> Self {
        let now = clock.now_ms();
        let status = AgentStatus {
            running: true,
            pid: Some(std::process::id()),
            since: Some(rfc3339_from_ms(now)),
            ..AgentStatus::default()
        };
        Self {
            ctx,
            clock,
            notifier,
            next_poll: now,
            next_check: now + cfg.startup_delay_ms,
            next_enforce: now + cfg.startup_delay_ms,
            next_schedule: now + cfg.startup_delay_ms,
            cfg,
            cancel: CancelToken::new(),
            watch: HashMap::new(),
            last_notified_at: None,
            last_notified_bytes: 0,
            status,
            dirty: true,
        }
    }

    /// Cancelling this stops a running clean / scan promptly (shutdown).
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    pub fn status(&self) -> &AgentStatus {
        &self.status
    }

    fn job(&self) -> Job {
        Job::with_token(self.cancel.clone())
    }

    fn act(&mut self, text: String) {
        self.status.last_action = Some(text);
        self.status.last_action_at = Some(rfc3339_from_ms(self.clock.now_ms()));
        self.dirty = true;
    }

    fn notify(&self, s: &Settings, title: &str, body: &str, ev: &mut Vec<Event>) {
        if s.smart.notify {
            self.notifier.notify(title, body);
            ev.push(Event::Notified {
                title: title.to_string(),
                body: body.to_string(),
            });
        }
    }

    /// Run whatever is due now.
    pub fn tick(&mut self) -> Vec<Event> {
        let now = self.clock.now_ms();
        let mut ev = Vec::new();
        if now >= self.next_poll
            || now >= self.next_check
            || now >= self.next_enforce
            || now >= self.next_schedule
        {
            let s = settings::load(&self.ctx);
            if now >= self.next_poll {
                self.next_poll = now + self.cfg.poll_ms;
                self.poll_browsers(&s, &mut ev);
            }
            if now >= self.next_check && !self.cancel.is_cancelled() {
                self.next_check = now + s.smart.check_interval_minutes.max(5) as u64 * 60_000;
                self.check_junk(&s, &mut ev);
            }
            if now >= self.next_enforce && !self.cancel.is_cancelled() {
                self.next_enforce = now + s.smart.enforce_sleep_minutes.max(1) as u64 * 60_000;
                self.enforce(&mut ev);
            }
            if now >= self.next_schedule && !self.cancel.is_cancelled() {
                self.next_schedule = now + SCHEDULE_POLL_MS;
                self.catch_up(&mut ev);
            }
        }
        self.flush();
        ev
    }

    /// One iteration of every task, ignoring cadence (`clearsweep agent --once`, tests).
    /// Browsers are polled `polls` times back to back so a close can be observed.
    pub fn run_once(&mut self, polls: u32) -> Vec<Event> {
        let s = settings::load(&self.ctx);
        let mut ev = Vec::new();
        for _ in 0..polls.max(1) {
            self.poll_browsers(&s, &mut ev);
        }
        self.check_junk(&s, &mut ev);
        self.enforce(&mut ev);
        self.catch_up(&mut ev);
        self.flush();
        ev
    }

    /// Mark the agent as stopped in the status file.
    pub fn shutdown(&mut self) {
        self.status.running = false;
        self.status.pid = None;
        write_status(&self.ctx, &self.status);
        self.dirty = false;
    }

    fn flush(&mut self) {
        if self.dirty {
            write_status(&self.ctx, &self.status);
            self.dirty = false;
        }
    }

    // ------------------------------------------------------------ junk

    fn check_junk(&mut self, s: &Settings, ev: &mut Vec<Event>) {
        if !s.smart.enabled {
            return;
        }
        let now = self.clock.now_ms();
        let res = match smart_cleaning::check_junk(&self.ctx, s, &self.job()) {
            Ok(r) => r,
            Err(e) => {
                if e.code != crate::error::ErrorCode::Cancelled {
                    ev.push(Event::Error(format!("junk check failed: {}", e.message)));
                }
                return;
            }
        };
        self.status.last_check = Some(rfc3339_from_ms(now));
        self.status.last_junk_bytes = Some(res.junk_bytes);
        self.dirty = true;
        ev.push(Event::JunkChecked {
            bytes: res.junk_bytes,
            over_threshold: res.over_threshold,
        });
        if !res.over_threshold {
            return;
        }
        if let Some(c) = &res.cleaned {
            ev.push(Event::AutoCleaned {
                bytes: c.removed_bytes,
            });
            self.act(format!("Cleaned {}", human_bytes(c.removed_bytes)));
            if c.removed_bytes > 0 {
                self.notify(
                    s,
                    "ClearSweep",
                    &format!("Cleaned {}", human_bytes(c.removed_bytes)),
                    ev,
                );
            }
            return;
        }
        if let Some(err) = &res.clean_error {
            ev.push(Event::Error(format!("automatic clean failed: {err}")));
        }
        if !s.smart.notify {
            return;
        }
        let due = match self.last_notified_at {
            None => true,
            Some(at) => {
                now.saturating_sub(at) >= NOTIFY_GAP_MS
                    || res.junk_bytes as f64 > self.last_notified_bytes as f64 * NOTIFY_GROWTH
            }
        };
        if due {
            self.last_notified_at = Some(now);
            self.last_notified_bytes = res.junk_bytes;
            let body = format!("ClearSweep found {} of junk", human_bytes(res.junk_bytes));
            self.notify(s, "ClearSweep", &body, ev);
            self.act(body);
        }
    }

    // ------------------------------------------------------------ browsers

    fn poll_browsers(&mut self, s: &Settings, ev: &mut Vec<Event>) {
        if !s.smart.enabled || s.smart.clean_on_browser_close.is_empty() {
            self.watch.clear();
            return;
        }
        let os = self.ctx.env.os;
        let procs: Vec<ProcInfo> = self.ctx.procs.list();
        let rules = cleaner::available_rules(&self.ctx, s);
        self.watch
            .retain(|g, _| s.smart.clean_on_browser_close.contains(g));
        let mut to_clean: Vec<String> = Vec::new();
        for group in &s.smart.clean_on_browser_close {
            let mut in_group = rules
                .iter()
                .filter(|r| r.category == Category::Browser && &r.group == group)
                .peekable();
            if in_group.peek().is_none() {
                continue; // that browser has no rules on this system
            }
            let running = in_group.any(|r| apps::is_running(r, os, &procs));
            let w = self.watch.entry(group.clone()).or_default();
            if running {
                w.was_running = true;
                w.closed_polls = 0;
            } else if w.was_running {
                w.closed_polls += 1;
                if w.closed_polls >= CLOSED_POLLS {
                    w.was_running = false;
                    w.closed_polls = 0;
                    to_clean.push(group.clone());
                }
            }
        }
        for group in to_clean {
            self.clean_browser(s, &group, ev);
        }
    }

    fn clean_browser(&mut self, s: &Settings, group: &str, ev: &mut Vec<Event>) {
        let ids: Vec<String> = match cleaner::select_rules(&self.ctx, s, None) {
            Ok(r) => r
                .into_iter()
                .filter(|r| r.category == Category::Browser && r.group == group)
                .map(|r| r.id)
                .collect(),
            Err(e) => {
                ev.push(Event::Error(e.message));
                return;
            }
        };
        if ids.is_empty() {
            return;
        }
        let opts = CleanOptions::from_settings(s, true);
        match cleaner::run_clean(&self.ctx, Some(ids), &opts, Source::Smart, &self.job()) {
            Ok(r) => {
                ev.push(Event::BrowserCleaned {
                    group: group.to_string(),
                    bytes: r.total_bytes,
                });
                self.act(format!(
                    "Cleaned {} after {group} closed",
                    human_bytes(r.total_bytes)
                ));
                if r.total_bytes > 0 {
                    self.notify(
                        s,
                        "ClearSweep",
                        &format!(
                            "Cleaned {} after {group} closed",
                            human_bytes(r.total_bytes)
                        ),
                        ev,
                    );
                }
            }
            Err(e) => ev.push(Event::Error(format!(
                "cleaning {group} failed: {}",
                e.message
            ))),
        }
    }

    // ------------------------------------------------------------ enforce / schedules

    fn enforce(&mut self, ev: &mut Vec<Event>) {
        match crate::api::dispatch(&self.ctx, "optimizer.enforce", Value::Null, &self.job()) {
            Ok(v) => {
                let n = v
                    .get("changed")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                if n > 0 {
                    self.act(format!(
                        "Kept {n} startup item(s) of sleeping apps disabled"
                    ));
                }
                ev.push(Event::Enforced { changed: n });
            }
            Err(e) => ev.push(Event::Error(format!(
                "sleep-mode enforcement failed: {}",
                e.message
            ))),
        }
    }

    fn catch_up(&mut self, ev: &mut Vec<Event>) {
        let ran = crate::features::scheduler::catch_up(&self.ctx, self.clock.as_ref(), &self.job());
        for (id, r) in ran {
            match r {
                Ok(bytes) => {
                    ev.push(Event::ScheduleCaughtUp { id: id.clone() });
                    self.act(format!(
                        "Ran a missed scheduled clean ({})",
                        human_bytes(bytes)
                    ));
                }
                Err(e) => ev.push(Event::Error(format!("scheduled clean {id} failed: {e}"))),
            }
        }
    }
}

/// Drive `agent` until `stop` is set: tick about once a second.
pub fn run_loop(agent: &mut Agent, stop: &AtomicBool, mut on_events: impl FnMut(&[Event])) {
    while !stop.load(Ordering::SeqCst) {
        let ev = agent.tick();
        if !ev.is_empty() {
            on_events(&ev);
        }
        // Sleep in short slices so a stop request is honoured quickly.
        for _ in 0..10 {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    agent.shutdown();
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
