//! The agent state machine against a fake clock, scripted processes and a recording notifier.

use super::*;
use crate::clock::LocalTime;
use crate::features::cleaner::history;
use crate::procs::{ProcDetail, ProcessSource};
use crate::runner::MockRunner;
use crate::testutil::{Chromium, Fixture};
use std::sync::atomic::AtomicU64;
use std::sync::Mutex;

const MB: usize = 1024 * 1024;

struct FakeClock {
    ms: AtomicU64,
    local: Mutex<Option<LocalTime>>,
}

impl FakeClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            // 2024-01-03 12:00:00 UTC
            ms: AtomicU64::new(1_704_283_200_000),
            local: Mutex::new(None),
        })
    }
    fn advance(&self, ms: u64) {
        self.ms.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.ms.load(Ordering::SeqCst)
    }
    fn local(&self) -> Option<LocalTime> {
        *self.local.lock().unwrap()
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<(String, String)>>);

impl Notifier for Recorder {
    fn notify(&self, title: &str, body: &str) {
        self.0
            .lock()
            .unwrap()
            .push((title.to_string(), body.to_string()));
    }
}

impl Recorder {
    fn bodies(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().map(|n| n.1.clone()).collect()
    }
}

/// A process list the test can change between polls.
#[derive(Default)]
struct Procs(Mutex<Vec<String>>);

impl Procs {
    fn set(&self, names: &[&str]) {
        *self.0.lock().unwrap() = names.iter().map(|s| s.to_string()).collect();
    }
}

impl ProcessSource for Procs {
    fn list(&self) -> Vec<ProcInfo> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, n)| ProcInfo {
                pid: 100 + i as u32,
                name: n.clone(),
            })
            .collect()
    }
    fn details(&self) -> Vec<ProcDetail> {
        Vec::new()
    }
    fn request_exit(&self, _pid: u32) -> bool {
        false
    }
}

struct Bed {
    _tmp: tempfile::TempDir,
    fx: Fixture,
    ctx: Ctx,
    clock: Arc<FakeClock>,
    notes: Arc<Recorder>,
    procs: Arc<Procs>,
}

fn bed() -> Bed {
    let tmp = tempfile::tempdir().unwrap();
    let fx = Fixture::new(tmp.path());
    let procs = Arc::new(Procs::default());
    let ctx = Ctx::new(fx.env.clone(), Arc::new(MockRunner::new())).with_procs(procs.clone());
    Bed {
        _tmp: tmp,
        fx,
        ctx,
        clock: FakeClock::new(),
        notes: Arc::new(Recorder::default()),
        procs,
    }
}

impl Bed {
    fn agent(&self, startup_delay_ms: u64) -> Agent {
        Agent::with_config(
            self.ctx.clone(),
            self.clock.clone(),
            self.notes.clone(),
            AgentConfig {
                startup_delay_ms,
                poll_ms: POLL_MS,
            },
        )
    }
    fn set(&self, f: impl FnOnce(&mut Settings)) {
        settings::update(&self.ctx, f).unwrap();
    }
    /// Chrome cache with `mb` megabytes of junk.
    fn chrome_junk(&self, mb: usize) -> std::path::PathBuf {
        let p = self.fx.chromium_profile(Chromium::Chrome, "Default");
        self.fx.file(p.cache.join("Cache/Cache_Data/big"), mb * MB);
        p.cache.join("Cache/Cache_Data/big")
    }
    fn smart(&self, f: impl FnOnce(&mut crate::features::settings::SmartSettings)) {
        self.set(|s| {
            s.selected_rules = Some(vec!["chrome.cache".into()]);
            s.smart.enabled = true;
            s.smart.threshold_mb = 1;
            f(&mut s.smart);
        });
    }
}

fn notified(ev: &[Event]) -> usize {
    ev.iter()
        .filter(|e| matches!(e, Event::Notified { .. }))
        .count()
}

// ---------------------------------------------------------------- junk threshold

#[test]
fn over_threshold_notifies_once_then_rate_limits() {
    let b = bed();
    b.chrome_junk(2);
    b.smart(|s| s.check_interval_minutes = 5);
    let mut a = b.agent(0);

    let ev = a.tick();
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::JunkChecked { over_threshold: true, bytes } if *bytes >= 2 * MB as u64)),
        "{ev:?}"
    );
    assert_eq!(notified(&ev), 1);
    assert_eq!(b.notes.bodies().len(), 1);
    assert!(
        b.notes.bodies()[0].starts_with("ClearSweep found 2.0 MB of junk"),
        "{:?}",
        b.notes.bodies()
    );

    // Five minutes later: checked again, but no second notification.
    b.clock.advance(5 * 60_000);
    let ev = a.tick();
    assert!(ev.iter().any(|e| matches!(e, Event::JunkChecked { .. })));
    assert_eq!(notified(&ev), 0);

    // Eleven and a half hours after the first: still limited.
    b.clock.advance(11 * 3_600_000 + 20 * 60_000);
    assert_eq!(notified(&a.tick()), 0);

    // Past twelve hours: notifies again.
    b.clock.advance(40 * 60_000);
    assert_eq!(notified(&a.tick()), 1);
    assert_eq!(b.notes.bodies().len(), 2);
}

#[test]
fn junk_growth_of_more_than_half_breaks_the_rate_limit() {
    let b = bed();
    b.chrome_junk(2);
    b.smart(|s| s.check_interval_minutes = 5);
    let mut a = b.agent(0);
    assert_eq!(notified(&a.tick()), 1);

    // 2 MB -> 3 MB is exactly +50%: not more than 50%.
    let p = b.fx.chromium_profile(Chromium::Chrome, "Default");
    b.fx.file(p.cache.join("Cache/Cache_Data/more"), MB);
    b.clock.advance(5 * 60_000);
    let ev = a.tick();
    let bytes = ev
        .iter()
        .find_map(|e| match e {
            Event::JunkChecked { bytes, .. } => Some(*bytes),
            _ => None,
        })
        .unwrap();
    assert_eq!(bytes, 3 * MB as u64, "test setup");
    assert_eq!(
        notified(&ev),
        0,
        "{bytes} bytes is not more than +50% of 2 MB"
    );

    // Add enough to clear the +50% line.
    b.fx.file(p.cache.join("Cache/Cache_Data/more2"), 2 * MB);
    b.clock.advance(5 * 60_000);
    assert_eq!(notified(&a.tick()), 1);
}

#[test]
fn under_threshold_or_notify_off_or_disabled_stays_quiet() {
    let b = bed();
    b.chrome_junk(2);
    // Threshold above the junk.
    b.smart(|s| s.threshold_mb = 50);
    let mut a = b.agent(0);
    let ev = a.tick();
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::JunkChecked {
            over_threshold: false,
            ..
        }
    )));
    assert_eq!(notified(&ev), 0);

    // Over the threshold but notifications are off.
    b.smart(|s| {
        s.threshold_mb = 1;
        s.notify = false;
    });
    b.clock.advance(61 * 60_000);
    let ev = a.tick();
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::JunkChecked {
            over_threshold: true,
            ..
        }
    )));
    assert!(b.notes.bodies().is_empty());

    // Smart cleaning off: nothing is even measured.
    b.set(|s| {
        s.smart.enabled = false;
        s.smart.notify = true;
    });
    b.clock.advance(61 * 60_000);
    let ev = a.tick();
    assert!(!ev.iter().any(|e| matches!(e, Event::JunkChecked { .. })));
}

#[test]
fn auto_clean_removes_the_junk_and_says_so() {
    let b = bed();
    let big = b.chrome_junk(2);
    let p = b.fx.chromium_profile(Chromium::Chrome, "Default");
    let keep = b.fx.file(p.data.join("Bookmarks"), 300);
    b.smart(|s| s.auto_clean = true);
    let mut a = b.agent(0);

    let ev = a.tick();
    assert!(!big.exists(), "the cache was cleaned");
    assert!(keep.exists());
    assert!(ev
        .iter()
        .any(|e| matches!(e, Event::AutoCleaned { bytes } if *bytes >= 2 * MB as u64)));
    let bodies = b.notes.bodies();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(bodies[0].starts_with("Cleaned "), "{bodies:?}");
    assert!(
        !bodies[0].contains("found"),
        "no separate 'found' notice: {bodies:?}"
    );
    // History records who did it.
    let h = history::list(&b.ctx, Some(1));
    assert_eq!(h[0].source, Source::Smart);
    // The status line says what happened.
    assert!(a
        .status()
        .last_action
        .as_deref()
        .unwrap()
        .starts_with("Cleaned "));

    // Next check finds nothing over the threshold.
    b.clock.advance(61 * 60_000);
    let ev = a.tick();
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::JunkChecked {
            over_threshold: false,
            ..
        }
    )));
}

#[test]
fn auto_clean_is_silent_when_notify_is_off() {
    let b = bed();
    let big = b.chrome_junk(2);
    b.smart(|s| {
        s.auto_clean = true;
        s.notify = false;
    });
    let mut a = b.agent(0);
    let ev = a.tick();
    assert!(!big.exists());
    assert!(ev.iter().any(|e| matches!(e, Event::AutoCleaned { .. })));
    assert!(b.notes.bodies().is_empty());
}

#[test]
fn checks_follow_the_configured_interval() {
    let b = bed();
    b.chrome_junk(2);
    b.smart(|s| {
        s.check_interval_minutes = 30;
        s.notify = false;
    });
    let mut a = b.agent(0);
    let checks = |ev: &[Event]| {
        ev.iter()
            .filter(|e| matches!(e, Event::JunkChecked { .. }))
            .count()
    };
    assert_eq!(checks(&a.tick()), 1);
    b.clock.advance(29 * 60_000);
    assert_eq!(checks(&a.tick()), 0);
    b.clock.advance(60_000);
    assert_eq!(checks(&a.tick()), 1);
}

#[test]
fn the_first_check_waits_for_the_startup_delay() {
    let b = bed();
    b.chrome_junk(2);
    b.smart(|_| {});
    let mut a = b.agent(30_000);
    assert!(a.tick().is_empty(), "nothing yet");
    b.clock.advance(29_000);
    assert!(a.tick().is_empty());
    b.clock.advance(1_000);
    assert!(a
        .tick()
        .iter()
        .any(|e| matches!(e, Event::JunkChecked { .. })));
}

// ---------------------------------------------------------------- browsers

fn browser_bed() -> (Bed, std::path::PathBuf, std::path::PathBuf) {
    let b = bed();
    let chrome = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let ff = b.fx.populate_firefox("abc.default");
    b.set(|s| {
        s.selected_rules = Some(vec![
            "chrome.cache".into(),
            "firefox.cache".into(),
            "chrome.history".into(),
        ]);
        s.smart.enabled = true;
        s.smart.threshold_mb = 10_000;
        s.smart.notify = true;
        s.smart.clean_on_browser_close = vec!["Google Chrome".into()];
    });
    (
        b,
        chrome.cache.join("Cache/Cache_Data/data_0"),
        ff.cache.join("cache2/entries/AAAA1111"),
    )
}

fn poll(a: &mut Agent, b: &Bed, names: &[&str]) -> Vec<Event> {
    b.procs.set(names);
    b.clock.advance(POLL_MS);
    a.tick()
}

#[test]
fn browser_close_is_debounced_and_cleans_only_that_browser() {
    let (b, chrome_file, firefox_file) = browser_bed();
    // Long startup delay: only browser polling is under test here.
    let mut a = b.agent(3_600_000);
    b.procs.set(&["chrome", "firefox"]);
    assert!(a.tick().is_empty(), "first poll only records the baseline");
    assert!(chrome_file.exists());

    // Chrome vanishes: one poll is not enough.
    let ev = poll(&mut a, &b, &["firefox"]);
    assert!(ev.is_empty(), "{ev:?}");
    assert!(chrome_file.exists());

    // It comes back (a relaunch): the counter resets.
    assert!(poll(&mut a, &b, &["chrome", "firefox"]).is_empty());
    assert!(poll(&mut a, &b, &["firefox"]).is_empty());
    assert!(chrome_file.exists());

    // Closed for two consecutive polls: cleaned.
    let ev = poll(&mut a, &b, &["firefox"]);
    assert!(
        ev.iter().any(|e| matches!(e, Event::BrowserCleaned { group, bytes } if group == "Google Chrome" && *bytes > 0)),
        "{ev:?}"
    );
    assert!(!chrome_file.exists(), "chrome's cache was cleaned");
    assert!(
        firefox_file.exists(),
        "firefox was not touched (it is still running and not selected)"
    );
    let bodies = b.notes.bodies();
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0].contains("after Google Chrome closed"),
        "{bodies:?}"
    );
    assert_eq!(history::list(&b.ctx, Some(1))[0].source, Source::Smart);
    // Chrome's history rule was in the selection too.
    let ids = &history::list(&b.ctx, Some(1))[0].rule_ids;
    assert!(ids.contains(&"chrome.cache".to_string()), "{ids:?}");
    assert!(!ids.iter().any(|i| i.starts_with("firefox")), "{ids:?}");

    // Nothing more happens while it stays closed.
    assert!(poll(&mut a, &b, &["firefox"]).is_empty());
    assert!(poll(&mut a, &b, &["firefox"]).is_empty());
}

#[test]
fn a_browser_that_was_never_seen_running_is_not_cleaned() {
    let (b, chrome_file, _) = browser_bed();
    let mut a = b.agent(3_600_000);
    for _ in 0..5 {
        assert!(poll(&mut a, &b, &[]).is_empty());
    }
    assert!(chrome_file.exists());
}

#[test]
fn browsers_not_listed_in_the_settings_are_ignored() {
    let (b, chrome_file, firefox_file) = browser_bed();
    b.set(|s| s.smart.clean_on_browser_close = vec!["Mozilla Firefox".into()]);
    let mut a = b.agent(3_600_000);
    b.procs.set(&["chrome", "firefox"]);
    a.tick();
    poll(&mut a, &b, &[]);
    let ev = poll(&mut a, &b, &[]);
    assert!(
        ev.iter().any(
            |e| matches!(e, Event::BrowserCleaned { group, .. } if group == "Mozilla Firefox")
        ),
        "{ev:?}"
    );
    assert!(
        chrome_file.exists(),
        "chrome closed too but is not selected"
    );
    assert!(!firefox_file.exists());
}

#[test]
fn unknown_or_disabled_browser_watching_does_nothing() {
    let (b, chrome_file, _) = browser_bed();
    b.set(|s| s.smart.clean_on_browser_close = vec!["Netscape".into()]);
    let mut a = b.agent(3_600_000);
    b.procs.set(&["chrome"]);
    a.tick();
    poll(&mut a, &b, &[]);
    assert!(poll(&mut a, &b, &[]).is_empty());
    assert!(chrome_file.exists());

    b.set(|s| {
        s.smart.clean_on_browser_close = vec!["Google Chrome".into()];
        s.smart.enabled = false;
    });
    b.procs.set(&["chrome"]);
    poll(&mut a, &b, &["chrome"]);
    poll(&mut a, &b, &[]);
    assert!(poll(&mut a, &b, &[]).is_empty());
    assert!(chrome_file.exists());
}

#[test]
fn a_browser_that_comes_back_within_the_debounce_window_is_not_cleaned() {
    let (b, chrome_file, _) = browser_bed();
    let mut a = b.agent(3_600_000);
    b.procs.set(&["chrome"]);
    a.tick();
    poll(&mut a, &b, &[]);
    // Chrome reappears exactly when the second closed poll would have fired.
    assert!(poll(&mut a, &b, &["chrome"]).is_empty());
    assert!(chrome_file.exists());
    // And the count starts over afterwards.
    assert!(poll(&mut a, &b, &[]).is_empty());
    assert!(chrome_file.exists());
}

// ---------------------------------------------------------------- enforce + once

#[test]
fn enforce_runs_on_its_own_cadence() {
    let b = bed();
    b.set(|s| s.smart.enforce_sleep_minutes = 15);
    let mut a = b.agent(0);
    let enforced = |ev: &[Event]| {
        ev.iter()
            .filter(|e| matches!(e, Event::Enforced { .. }))
            .count()
    };
    assert_eq!(enforced(&a.tick()), 1);
    b.clock.advance(14 * 60_000);
    assert_eq!(enforced(&a.tick()), 0);
    b.clock.advance(60_000);
    assert_eq!(enforced(&a.tick()), 1);
    // The setting is re-read: 1 minute now.
    b.set(|s| s.smart.enforce_sleep_minutes = 1);
    b.clock.advance(15 * 60_000);
    assert_eq!(enforced(&a.tick()), 1);
    b.clock.advance(60_000);
    assert_eq!(enforced(&a.tick()), 1);
}

#[test]
fn run_once_runs_every_task_regardless_of_cadence() {
    let (b, chrome_file, _) = browser_bed();
    b.smart(|s| {
        s.threshold_mb = 10_000;
        s.clean_on_browser_close = vec!["Google Chrome".into()];
    });
    let mut a = b.agent(3_600_000);
    // Chrome is gone from the very first poll: never seen running, so not cleaned...
    let ev = a.run_once(3);
    assert!(ev.iter().any(|e| matches!(e, Event::JunkChecked { .. })));
    assert!(ev.iter().any(|e| matches!(e, Event::Enforced { .. })));
    assert!(chrome_file.exists());
    // ...and the normal cadence is untouched by run_once's forced pass.
    assert!(a
        .tick()
        .iter()
        .all(|e| !matches!(e, Event::JunkChecked { .. })));
}

// ---------------------------------------------------------------- lock + status

#[test]
fn the_lock_admits_one_agent_at_a_time() {
    let b = bed();
    assert!(!agent_running(&b.ctx));
    let first = AgentLock::try_acquire(&b.ctx)
        .unwrap()
        .expect("first agent gets the lock");
    assert!(agent_running(&b.ctx));
    assert!(
        AgentLock::try_acquire(&b.ctx).unwrap().is_none(),
        "a second agent must not start"
    );
    drop(first);
    assert!(!agent_running(&b.ctx));
    assert!(AgentLock::try_acquire(&b.ctx).unwrap().is_some());
}

#[test]
fn the_status_file_is_written_and_running_is_checked_against_the_lock() {
    let b = bed();
    b.chrome_junk(2);
    b.smart(|s| s.notify = false);
    // No agent has ever run.
    let s = read_status(&b.ctx);
    assert!(!s.running && s.since.is_none());

    let lock = AgentLock::try_acquire(&b.ctx).unwrap().unwrap();
    let mut a = b.agent(0);
    a.tick();
    let s = read_status(&b.ctx);
    assert!(s.running);
    assert_eq!(s.pid, Some(std::process::id()));
    assert_eq!(s.since.as_deref(), Some("2024-01-03T12:00:00Z"));
    assert_eq!(s.last_check.as_deref(), Some("2024-01-03T12:00:00Z"));
    assert!(s.last_junk_bytes.unwrap() >= 2 * MB as u64);
    // The file is JSON with camelCase keys, written atomically (no temp files left).
    let text = fs::read_to_string(b.ctx.env.data_dir.join(STATUS_FILE)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["running"], true);
    assert!(v["lastCheck"].is_string());
    let leftovers: Vec<_> = fs::read_dir(&b.ctx.env.data_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // Graceful shutdown.
    a.shutdown();
    drop(lock);
    let s = read_status(&b.ctx);
    assert!(!s.running && s.pid.is_none());
    assert!(s.since.is_some(), "history of the last run is kept");
}

#[test]
fn a_crashed_agents_leftover_status_does_not_claim_to_be_running() {
    let b = bed();
    let mut a = b.agent(0);
    a.tick(); // writes running: true
    let raw: serde_json::Value =
        serde_json::from_slice(&fs::read(b.ctx.env.data_dir.join(STATUS_FILE)).unwrap()).unwrap();
    assert_eq!(raw["running"], true);
    // No lock is held (the process "crashed"): the verified status says stopped.
    assert!(!read_status(&b.ctx).running);
}

#[test]
fn run_loop_stops_on_request_and_marks_the_agent_stopped() {
    let b = bed();
    let mut a = b.agent(0);
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        s2.store(true, Ordering::SeqCst);
    });
    run_loop(&mut a, &stop, |_| {});
    t.join().unwrap();
    let raw: serde_json::Value =
        serde_json::from_slice(&fs::read(b.ctx.env.data_dir.join(STATUS_FILE)).unwrap()).unwrap();
    assert_eq!(raw["running"], false);
}

#[test]
fn shutdown_cancels_a_running_scan() {
    let b = bed();
    b.chrome_junk(2);
    b.smart(|_| {});
    let mut a = b.agent(0);
    a.cancel_token().cancel();
    let ev = a.tick();
    assert!(
        !ev.iter()
            .any(|e| matches!(e, Event::Error(_) | Event::JunkChecked { .. })),
        "{ev:?}"
    );
}
