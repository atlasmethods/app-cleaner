#![cfg(unix)]
//! Real-process checks of the background agent, scheduled cleans and run-at-startup, in a
//! sandbox HOME. Nothing here can reach the real user's session: PATH holds only fake
//! `systemctl` / `crontab` scripts (or nothing), and notifications are written to a file.

use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
use sweep_core::testutil::{Chromium, Fixture};

const BIN: &str = env!("CARGO_BIN_EXE_clearsweep");

struct Box_ {
    dir: tempfile::TempDir,
    fx: Fixture,
    /// Directory that is the whole PATH of every command.
    path_dir: PathBuf,
}

fn sandbox() -> Box_ {
    let dir = tempfile::tempdir().unwrap();
    let fx = Fixture::new(dir.path());
    let path_dir = dir.path().join("fake-bin");
    fs::create_dir_all(&path_dir).unwrap();
    fs::create_dir_all(dir.path().join("state")).unwrap();
    Box_ { dir, fx, path_dir }
}

impl Box_ {
    fn base(&self) -> &Path {
        self.dir.path()
    }
    fn cmd(&self, seq: Option<&str>) -> Command {
        let home = self.base().join("home");
        let mut c = Command::new(BIN);
        c.env_clear()
            .env("PATH", &self.path_dir)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("CLEARSWEEP_ROOT", self.base().join("root"))
            .env("CLEARSWEEP_DATA_DIR", self.base().join("data"))
            .env("CLEARSWEEP_EXE", BIN)
            .env("CLEARSWEEP_NOTIFY_FILE", self.base().join("notes.txt"))
            .env("CLEARSWEEP_FAKE_PROCESSES", "")
            .env("FAKE_STATE", self.base().join("state"))
            .env("TMPDIR", self.base().join("root/tmp"));
        if let Some(s) = seq {
            c.env("CLEARSWEEP_FAKE_PROCESSES_SEQ", s);
        }
        c
    }
    fn call(&self, method: &str, params: Value) -> Value {
        let out = self
            .cmd(None)
            .args(["call", method, &params.to_string()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{method}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn fake(&self, name: &str, script: &str) {
        let p = self.path_dir.join(name);
        fs::write(&p, format!("#!/bin/sh\n{script}")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    /// Fake `systemctl` (user manager works) and `crontab`, logging to state/log.txt.
    fn fake_scheduler_tools(&self) {
        self.fake(
            "systemctl",
            "PATH=/usr/bin:/bin\necho \"systemctl $*\" >> \"$FAKE_STATE/log.txt\"\nexit 0\n",
        );
    }
    fn fake_crontab(&self) {
        self.fake(
            "crontab",
            "PATH=/usr/bin:/bin\necho \"crontab $*\" >> \"$FAKE_STATE/log.txt\"\nif [ \"$1\" = \"-l\" ]; then\n  if [ -f \"$FAKE_STATE/crontab\" ]; then cat \"$FAKE_STATE/crontab\"; exit 0; fi\n  echo 'no crontab for user' >&2; exit 1\nfi\ncp \"$1\" \"$FAKE_STATE/crontab\"\n",
        );
    }
    fn notes(&self) -> String {
        fs::read_to_string(self.base().join("notes.txt")).unwrap_or_default()
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

// ---------------------------------------------------------------- agent

#[test]
fn agent_once_cleans_only_the_browser_that_closed() {
    let b = sandbox();
    let chrome = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let ff = b.fx.populate_firefox("abc.default");
    b.call(
        "settings.set",
        json!({
            "selectedRules": ["chrome.cache", "firefox.cache"],
            "smart": {"enabled": true, "thresholdMb": 1000, "cleanOnBrowserClose": ["Google Chrome"]}
        }),
    );
    let chrome_file = chrome.cache.join("Cache/Cache_Data/data_0");
    let ff_file = ff.cache.join("cache2/entries/AAAA1111");

    // Chrome running, then closed for two polls; Firefox runs throughout.
    let out = b
        .cmd(Some("chrome,firefox;firefox;firefox"))
        .args(["agent", "--once"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        !chrome_file.exists(),
        "chrome's cache was cleaned: {}",
        text(&out)
    );
    assert!(ff_file.exists(), "firefox was not touched");
    assert!(chrome.data.join("Bookmarks").exists());
    assert!(
        text(&out).contains("after Google Chrome closed"),
        "{}",
        text(&out)
    );
    assert!(b.notes().contains("Cleaned"), "{}", b.notes());
    let h = b.call("cleaner.history", json!({"limit": 1}));
    assert_eq!(h[0]["source"], "smart");
    assert_eq!(h[0]["ruleIds"], json!(["chrome.cache"]));

    // Status file written and released.
    let s = b.call("smart_cleaning.status", Value::Null);
    assert_eq!(s["running"], false);
    assert!(s["lastAction"].as_str().unwrap().contains("Google Chrome"));
}

#[test]
fn agent_once_does_not_clean_a_browser_that_never_ran() {
    let b = sandbox();
    let chrome = b.fx.populate_chromium(Chromium::Chrome, "Default");
    b.call(
        "settings.set",
        json!({
            "selectedRules": ["chrome.cache"],
            "smart": {"enabled": true, "thresholdMb": 1000, "cleanOnBrowserClose": ["Google Chrome"]}
        }),
    );
    let out = b
        .cmd(Some(";;"))
        .args(["agent", "--once"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(chrome.cache.join("Cache/Cache_Data/data_0").exists());
}

#[test]
fn agent_once_notifies_and_auto_cleans_past_the_threshold() {
    let b = sandbox();
    let chrome = b.fx.populate_chromium(Chromium::Chrome, "Default");
    b.fx.file(chrome.cache.join("Cache/Cache_Data/big"), 2 * 1024 * 1024);
    // Notify only: reported once, nothing deleted.
    b.call(
        "settings.set",
        json!({"selectedRules": ["chrome.cache"], "smart": {"enabled": true, "thresholdMb": 1}}),
    );
    let out = b.cmd(None).args(["agent", "--once"]).output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        b.notes().contains("ClearSweep found 2.1 MB of junk"),
        "{}",
        b.notes()
    );
    assert!(chrome.cache.join("Cache/Cache_Data/big").exists());
    // With auto clean: removed.
    b.call("settings.set", json!({"smart": {"autoClean": true}}));
    let out = b.cmd(None).args(["agent", "--once"]).output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(!chrome.cache.join("Cache/Cache_Data/big").exists());
    assert!(b.notes().contains("Cleaned 2.1 MB"), "{}", b.notes());
}

#[test]
fn a_second_agent_does_nothing_while_the_lock_is_held() {
    let b = sandbox();
    let chrome = b.fx.populate_chromium(Chromium::Chrome, "Default");
    b.fx.file(chrome.cache.join("Cache/Cache_Data/big"), 2 * 1024 * 1024);
    b.call(
        "settings.set",
        json!({"selectedRules": ["chrome.cache"], "smart": {"enabled": true, "thresholdMb": 1, "autoClean": true}}),
    );
    let ctx = sweep_core::Ctx::new(
        b.fx.env.clone(),
        std::sync::Arc::new(sweep_core::SystemRunner),
    );
    let lock = sweep_core::agent::AgentLock::try_acquire(&ctx)
        .unwrap()
        .unwrap();
    assert_eq!(
        b.call("smart_cleaning.status", Value::Null)["running"],
        true
    );
    let out = b.cmd(None).args(["agent", "--once"]).output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("already running"), "{}", text(&out));
    assert!(
        chrome.cache.join("Cache/Cache_Data/big").exists(),
        "the second agent did nothing"
    );
    drop(lock);
    let out = b.cmd(None).args(["agent", "--once"]).output().unwrap();
    assert!(out.status.success());
    assert!(!chrome.cache.join("Cache/Cache_Data/big").exists());
}

#[test]
fn the_agent_loop_runs_until_sigterm_then_releases_the_lock() {
    let b = sandbox();
    let mut child = b
        .cmd(None)
        .arg("agent")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let status_file = b.base().join("data/agent-status.json");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !status_file.exists() {
        assert!(
            Instant::now() < deadline,
            "the agent never wrote its status"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let s = b.call("smart_cleaning.status", Value::Null);
    assert_eq!(s["running"], true);
    assert_eq!(s["pid"], child.id());
    // A second agent exits immediately.
    let second = b.cmd(None).arg("agent").output().unwrap();
    assert!(second.status.success());
    assert!(text(&second).contains("already running"));

    let kill = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(kill.success());
    let deadline = Instant::now() + Duration::from_secs(15);
    let code = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        assert!(Instant::now() < deadline, "the agent ignored SIGTERM");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(code.success(), "clean exit, got {code:?}");
    let s = b.call("smart_cleaning.status", Value::Null);
    assert_eq!(s["running"], false);
    let raw: Value = serde_json::from_slice(&fs::read(&status_file).unwrap()).unwrap();
    assert_eq!(raw["running"], false);
    // The lock is free again.
    let out = b.cmd(None).args(["agent", "--once"]).output().unwrap();
    assert!(!text(&out).contains("already running"), "{}", text(&out));
}

#[test]
fn a_missing_notification_daemon_is_not_fatal() {
    let b = sandbox();
    let chrome = b.fx.populate_chromium(Chromium::Chrome, "Default");
    b.fx.file(chrome.cache.join("Cache/Cache_Data/big"), 2 * 1024 * 1024);
    b.call(
        "settings.set",
        json!({"selectedRules": ["chrome.cache"], "smart": {"enabled": true, "thresholdMb": 1}}),
    );
    // No CLEARSWEEP_NOTIFY_FILE, no session bus.
    let out = b
        .cmd(None)
        .env_remove("CLEARSWEEP_NOTIFY_FILE")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/bus")
        .args(["agent", "--once"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("ClearSweep found"), "{}", text(&out));
}

// ---------------------------------------------------------------- scheduled cleans

fn add_chrome_schedule(b: &Box_) -> String {
    let v = b.call(
        "scheduler.add",
        json!({"name": "Chrome nightly", "frequency": "daily", "time": "03:00",
               "action": {"kind": "clean", "rules": ["chrome.cache"]}}),
    );
    v["id"].as_str().unwrap().to_string()
}

#[test]
fn clean_with_schedule_runs_its_rules_and_records_last_run() {
    let b = sandbox();
    let chrome = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let id = add_chrome_schedule(&b);
    let out = b
        .cmd(None)
        .args([
            "clean",
            "--auto",
            "--source",
            "scheduled",
            "--schedule",
            &id,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(!chrome.cache.join("Cache/Cache_Data/data_0").exists());
    assert!(
        chrome.data.join("History").exists(),
        "only the schedule's rule ran"
    );

    let list = b.call("scheduler.list", Value::Null);
    assert!(list[0]["lastRun"].as_str().unwrap().ends_with('Z'));
    assert_eq!(list[0]["lastResult"]["ok"], true);
    assert_eq!(list[0]["lastResult"]["totalFiles"], 5);
    let h = b.call("cleaner.history", json!({"limit": 1}));
    assert_eq!(h[0]["source"], "scheduled");
}

#[test]
fn clean_with_schedule_edge_cases() {
    let b = sandbox();
    let chrome = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let id = add_chrome_schedule(&b);
    let file = chrome.cache.join("Cache/Cache_Data/data_0");

    // Disabled: the OS job that outlived the disable does nothing, successfully.
    b.call("scheduler.set_enabled", json!({"id": id, "enabled": false}));
    let out = b
        .cmd(None)
        .args(["clean", "--auto", "--schedule", &id])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(text(&out).contains("disabled"), "{}", text(&out));
    assert!(file.exists());

    // Unknown schedule: an error the OS job's log will show.
    let out = b
        .cmd(None)
        .args(["clean", "--auto", "--schedule", "0123456789abcdef"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(v["code"], "NotFound");
    assert!(file.exists());

    // --schedule and --rules together are refused by the argument parser.
    let out = b
        .cmd(None)
        .args([
            "clean",
            "--auto",
            "--schedule",
            &id,
            "--rules",
            "chrome.cache",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(file.exists());

    // --schedule needs --auto like every other clean.
    let out = b
        .cmd(None)
        .args(["clean", "--schedule", &id])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn a_failing_scheduled_run_is_recorded_and_exits_1() {
    let b = sandbox();
    b.fx.populate_chromium(Chromium::Chrome, "Default");
    let id = add_chrome_schedule(&b);
    // Break the schedule the way an edited file could.
    let path = b.base().join("data/schedules.json");
    let text_ = fs::read_to_string(&path)
        .unwrap()
        .replace("chrome.cache", "gone.rule");
    fs::write(&path, text_).unwrap();
    let out = b
        .cmd(None)
        .args(["clean", "--auto", "--schedule", &id])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let list = b.call("scheduler.list", Value::Null);
    assert_eq!(list[0]["lastResult"]["ok"], false);
    assert!(list[0]["lastResult"]["message"]
        .as_str()
        .unwrap()
        .contains("gone.rule"));
}

#[test]
fn schedule_add_writes_systemd_units_into_the_sandbox_home() {
    let b = sandbox();
    b.fake_scheduler_tools();
    let v = b.call(
        "scheduler.add",
        json!({"name": "Weekly", "frequency": "weekly", "time": "21:30", "weekdays": [1, 3]}),
    );
    let id = v["id"].as_str().unwrap();
    let dir = b.base().join("home/.config/systemd/user");
    let timer = fs::read_to_string(dir.join(format!("clearsweep-{id}.timer"))).unwrap();
    assert!(
        timer.contains("OnCalendar=Mon,Wed *-*-* 21:30:00"),
        "{timer}"
    );
    assert!(timer.contains("Persistent=true"));
    let service = fs::read_to_string(dir.join(format!("clearsweep-{id}.service"))).unwrap();
    assert!(
        service.contains(&format!(
            "ExecStart=\"{BIN}\" \"clean\" \"--auto\" \"--source\" \"scheduled\" \"--schedule\" \"{id}\""
        )),
        "{service}"
    );
    let log = fs::read_to_string(b.base().join("state/log.txt")).unwrap();
    assert!(log.contains("systemctl --user daemon-reload"), "{log}");
    assert!(
        log.contains(&format!(
            "systemctl --user enable --now clearsweep-{id}.timer"
        )),
        "{log}"
    );

    let backend = b.call("scheduler.backend", Value::Null);
    assert_eq!(backend["kind"], "systemd");

    b.call("scheduler.remove", json!({"id": id}));
    assert!(!dir.join(format!("clearsweep-{id}.timer")).exists());
    assert!(!dir.join(format!("clearsweep-{id}.service")).exists());
}

#[test]
fn schedule_add_falls_back_to_crontab_and_preserves_other_lines() {
    let b = sandbox();
    // systemctl exists but its user manager is not reachable (the situation in containers).
    b.fake("systemctl", "echo 'Failed to connect to bus' >&2\nexit 1\n");
    b.fake_crontab();
    fs::write(
        b.base().join("state/crontab"),
        "# mine\n*/5 * * * * /usr/bin/true\n@reboot /home/u/sync.sh\n",
    )
    .unwrap();
    let v = b.call(
        "scheduler.add",
        json!({"name": "Daily", "frequency": "daily", "time": "04:15"}),
    );
    let id = v["id"].as_str().unwrap();
    let ct = fs::read_to_string(b.base().join("state/crontab")).unwrap();
    assert!(
        ct.starts_with("# mine\n*/5 * * * * /usr/bin/true\n@reboot /home/u/sync.sh\n"),
        "{ct}"
    );
    assert!(
        ct.contains(&format!(
            "15 4 * * * {BIN} clean --auto --source scheduled --schedule {id} # clearsweep-schedule:{id}\n"
        )),
        "{ct}"
    );
    assert!(!b.base().join("home/.config/systemd").exists());
    assert_eq!(b.call("scheduler.backend", Value::Null)["kind"], "cron");

    // Disable removes the line; enable puts it back; remove leaves the original text.
    b.call("scheduler.set_enabled", json!({"id": id, "enabled": false}));
    assert_eq!(
        fs::read_to_string(b.base().join("state/crontab")).unwrap(),
        "# mine\n*/5 * * * * /usr/bin/true\n@reboot /home/u/sync.sh\n"
    );
    b.call("scheduler.set_enabled", json!({"id": id, "enabled": true}));
    assert!(fs::read_to_string(b.base().join("state/crontab"))
        .unwrap()
        .contains(id));
    b.call("scheduler.remove", json!({"id": id}));
    assert_eq!(
        fs::read_to_string(b.base().join("state/crontab")).unwrap(),
        "# mine\n*/5 * * * * /usr/bin/true\n@reboot /home/u/sync.sh\n"
    );
}

#[test]
fn without_systemd_or_cron_the_backend_is_unavailable_and_schedules_still_save() {
    let b = sandbox(); // empty PATH
    let backend = b.call("scheduler.backend", Value::Null);
    assert_eq!(backend["kind"], "none");
    assert_eq!(backend["available"], false);
    let id = add_chrome_schedule(&b);
    assert_eq!(b.call("scheduler.list", Value::Null)[0]["id"], id.as_str());
    assert!(!b.base().join("home/.config/systemd").exists());
    // Run now still works.
    let chrome = b.fx.populate_chromium(Chromium::Chrome, "Default");
    b.call("scheduler.run_now", json!({"id": id}));
    assert!(!chrome.cache.join("Cache/Cache_Data/data_0").exists());
}

#[test]
fn on_login_schedules_are_xdg_autostart_entries() {
    let b = sandbox();
    let v = b.call(
        "scheduler.add",
        json!({"name": "At login", "frequency": "on_login"}),
    );
    let id = v["id"].as_str().unwrap();
    let p = b.base().join(format!(
        "home/.config/autostart/clearsweep-schedule-{id}.desktop"
    ));
    let t = fs::read_to_string(&p).unwrap();
    assert!(
        t.contains(&format!(
            "Exec={BIN} clean --auto --source scheduled --schedule {id}"
        )),
        "{t}"
    );
    b.call("scheduler.remove", json!({"id": id}));
    assert!(!p.exists());
}

// ---------------------------------------------------------------- run at startup

#[test]
fn run_at_startup_installs_an_autostart_entry_that_runs_the_agent() {
    let b = sandbox();
    let entry = b
        .base()
        .join("home/.config/autostart/clearsweep-agent.desktop");
    let v = b.call("settings.set", json!({"runAtStartup": true}));
    assert_eq!(v["runAtStartup"], true);
    let t = fs::read_to_string(&entry).unwrap();
    assert!(t.contains(&format!("Exec={BIN} agent\n")), "{t}");
    assert!(t.contains("X-GNOME-Autostart-enabled=true") && t.contains("NoDisplay=true"));
    assert_eq!(b.call("settings.get", Value::Null)["runAtStartup"], true);
    // Removed by hand: settings.get tells the truth.
    fs::remove_file(&entry).unwrap();
    assert_eq!(b.call("settings.get", Value::Null)["runAtStartup"], false);
    b.call("settings.set", json!({"runAtStartup": true}));
    let v = b.call("settings.set", json!({"runAtStartup": false}));
    assert_eq!(v["runAtStartup"], false);
    assert!(!entry.exists());
}

#[test]
fn the_desktop_binary_name_contract_headless_commands() {
    // The desktop executable delegates exactly these subcommands to `sweep_cli::run`.
    for c in ["clean", "analyze", "agent", "call"] {
        assert!(sweep_cli::is_headless_command(c), "{c}");
    }
    for c in ["ui", "--hidden", "", "settings"] {
        assert!(!sweep_cli::is_headless_command(c), "{c}");
    }
}
