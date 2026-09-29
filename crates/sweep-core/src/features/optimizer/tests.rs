//! Optimizer tests: grouping, sleep / wake exactness, enforce, platform stop commands.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::ctx::{Env, Os};
use crate::error::ErrorCode;
use crate::features::startup::linux::user_autostart_dir;
use crate::procs::{FakeProcesses, ProcessSource};
use crate::runner::{CmdOutput, MockRunner};

const MB: u64 = 1024 * 1024;

fn job() -> Job {
    Job::detached()
}

fn proc(pid: u32, name: &str, exe: &str, mb: u64) -> ProcDetail {
    ProcDetail {
        pid,
        name: name.into(),
        exe: (!exe.is_empty()).then(|| exe.into()),
        memory_bytes: mb * MB,
        cpu_percent: 1.0,
        is_mine: true,
    }
}

struct Fx {
    _d: tempfile::TempDir,
    ctx: Ctx,
    procs: Arc<FakeProcesses>,
    runner: MockRunner,
}

fn fx(os: Os, procs: Vec<ProcDetail>, terminate: bool) -> Fx {
    let d = tempfile::tempdir().unwrap();
    let runner = MockRunner::new();
    let p = Arc::new(FakeProcesses::with_details(procs, terminate));
    let mut ctx = Ctx::new(Env::for_test(d.path()), Arc::new(runner.clone())).with_procs(p.clone());
    ctx.env.os = os;
    Fx {
        _d: d,
        ctx,
        procs: p,
        runner,
    }
}

fn write(p: &Path, text: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn autostart(f: &Fx, name: &str, text: &str) -> std::path::PathBuf {
    let p = user_autostart_dir(&f.ctx).join(name);
    write(&p, text);
    p
}

const SLACK: &str = "[Desktop Entry]\nType=Application\nName=Slack\nExec=/opt/Slack/slack -u %U\n";

fn call(f: &Fx, m: &str, p: Value) -> Result<Value> {
    crate::api::dispatch(&f.ctx, m, p, &job())
}

fn app<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["apps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["appId"] == id)
        .unwrap_or_else(|| panic!("no app {id} in {v}"))
}

fn app_ids(v: &Value) -> Vec<String> {
    v["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["appId"].as_str().unwrap().to_string())
        .collect()
}

fn slack_procs() -> Vec<ProcDetail> {
    vec![
        proc(100, "slack", "/opt/Slack/slack", 300),
        proc(101, "slack", "/opt/Slack/slack", 120),
        proc(
            102,
            "chrome_crashpad",
            "/opt/Slack/chrome_crashpad_handler",
            5,
        ),
    ]
}

fn state_json(f: &Fx) -> Value {
    serde_json::from_str(&fs::read_to_string(f.ctx.env.data_dir.join("optimizer.json")).unwrap())
        .unwrap()
}

// ---------------------------------------------------------------- analyze

#[test]
fn analyze_groups_processes_and_startup_items_per_app() {
    let mut procs = slack_procs();
    procs.push(proc(200, "bash", "/usr/bin/bash", 5));
    procs.push(proc(201, "gnome-shell", "/usr/bin/gnome-shell", 400));
    procs.push(proc(202, "clearsweep", "/usr/bin/clearsweep", 50));
    procs.push(proc(203, "systemd", "/usr/lib/systemd/systemd", 10));
    let mut foreign = proc(204, "slack", "/opt/Slack/slack", 999);
    foreign.is_mine = false;
    procs.push(foreign);
    procs.push(proc(205, "firefox", "/usr/lib/firefox/firefox", 500));
    let f = fx(Os::Linux, procs, true);
    autostart(&f, "slack.desktop", SLACK);
    write(
        &f.ctx
            .env
            .sys_path("/usr/share/applications/firefox.desktop"),
        "[Desktop Entry]\nName=Firefox\nExec=/usr/lib/firefox/firefox %u\nIcon=firefox\n",
    );
    let v = call(&f, "optimizer.analyze", json!({})).unwrap();
    let s = app(&v, "slack");
    assert_eq!(s["name"], "Slack");
    assert_eq!(s["startupIds"], json!(["xdg:user:slack.desktop"]));
    assert_eq!(s["serviceIds"], json!([]));
    let pids: Vec<u64> = s["processes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["pid"].as_u64().unwrap())
        .collect();
    assert_eq!(
        pids,
        vec![100, 101, 102],
        "sorted by memory, foreign pid 204 excluded"
    );
    assert_eq!(s["backgroundMemoryBytes"], (300 + 120 + 5) * MB);
    assert_eq!(s["sleeping"], false);
    assert_eq!(s["protected"], false);
    let ff = app(&v, "firefox");
    assert_eq!(ff["name"], "Firefox");
    assert_eq!(ff["icon"], "firefox");
    assert_eq!(ff["startupIds"], json!([]));
    // never listed: shells, the session, ourselves, other users' processes
    assert_eq!(app_ids(&v).len(), 2, "{:?}", app_ids(&v));
    assert_eq!(v["totals"]["apps"], 2);
    assert_eq!(v["totals"]["runningApps"], 2);
    assert_eq!(v["totals"]["backgroundMemoryBytes"], (425 + 500) * MB);
    assert_eq!(v["totals"]["startupItems"], 1);
}

#[test]
fn critical_items_are_not_apps_and_security_software_is_protected() {
    let f = fx(
        Os::Linux,
        vec![proc(
            10,
            "polkit-gnome-au",
            "/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1",
            20,
        )],
        true,
    );
    let sys = f
        .ctx
        .env
        .sys_path("/etc/xdg/autostart/polkit-gnome-authentication-agent-1.desktop");
    write(
        &sys,
        "[Desktop Entry]\nName=PolicyKit\nExec=/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1\n",
    );
    autostart(
        &f,
        "malwarebytes.desktop",
        "[Desktop Entry]\nName=Malwarebytes\nExec=/opt/malwarebytes/mbam\n",
    );
    let v = call(&f, "optimizer.analyze", json!({})).unwrap();
    assert_eq!(app_ids(&v), vec!["mbam"]);
    assert_eq!(app(&v, "mbam")["protected"], true);
}

#[test]
fn machine_wide_xdg_entries_are_eligible_because_they_need_no_root() {
    let f = fx(Os::Linux, vec![], true);
    write(
        &f.ctx.env.sys_path("/etc/xdg/autostart/dropbox.desktop"),
        "[Desktop Entry]\nName=Dropbox\nExec=/usr/bin/dropbox start\n",
    );
    let v = call(&f, "optimizer.analyze", json!({})).unwrap();
    assert_eq!(
        app(&v, "dropbox")["startupIds"],
        json!(["xdg:system:dropbox.desktop"])
    );
}

#[test]
fn flatpak_and_generic_launchers_do_not_merge_unrelated_apps() {
    let f = fx(Os::Linux, vec![], true);
    autostart(
        &f,
        "a.desktop",
        "[Desktop Entry]\nName=Alpha\nExec=flatpak run org.example.Alpha\n",
    );
    autostart(
        &f,
        "b.desktop",
        "[Desktop Entry]\nName=Beta\nExec=flatpak run --branch=stable org.example.Beta\n",
    );
    autostart(
        &f,
        "c.desktop",
        "[Desktop Entry]\nName=Gamma\nExec=env FOO=1 python3 /opt/g/g.py\n",
    );
    let v = call(&f, "optimizer.analyze", json!({})).unwrap();
    assert_eq!(app_ids(&v).len(), 3, "{:?}", app_ids(&v));
}

// ---------------------------------------------------------------- sleep / wake

#[test]
fn sleep_disables_startup_stops_processes_and_records_it() {
    let f = fx(Os::Linux, slack_procs(), true);
    let path = autostart(&f, "slack.desktop", SLACK);
    let r = call(&f, "optimizer.sleep", json!({"appIds": ["slack"]})).unwrap();
    let res = &r["results"][0];
    assert_eq!(res["ok"], true, "{res}");
    assert_eq!(res["disabledItems"], json!(["xdg:user:slack.desktop"]));
    assert_eq!(res["stoppedProcesses"], 3);
    assert_eq!(res["stillRunning"], 0);
    assert!(fs::read_to_string(&path).unwrap().contains("Hidden=true"));
    let mut req = f.procs.exit_requests();
    req.sort();
    assert_eq!(req, vec![100, 101, 102]);

    let saved = state_json(&f);
    let rec = &saved["sleeping"]["slack"];
    assert_eq!(rec["name"], "Slack");
    assert_eq!(rec["disabled"][0]["id"], "xdg:user:slack.desktop");
    assert_eq!(rec["alreadyDisabled"], json!([]));

    let v = call(&f, "optimizer.analyze", json!({})).unwrap();
    let a = app(&v, "slack");
    assert_eq!(a["sleeping"], true);
    assert_eq!(a["processes"], json!([]));
    assert_eq!(v["totals"]["sleepingApps"], 1);
}

#[test]
fn wake_restores_exactly_what_sleep_changed() {
    let f = fx(Os::Linux, slack_procs(), true);
    let one = autostart(&f, "slack.desktop", SLACK);
    // A second entry of the same app that the user had disabled long before.
    let two_text =
        "[Desktop Entry]\nName=Slack (tray)\nExec=/opt/Slack/slack --tray\nHidden=true\n";
    let two = autostart(&f, "slack-tray.desktop", two_text);
    call(&f, "optimizer.sleep", json!({"appIds": ["slack"]})).unwrap();
    let saved = state_json(&f);
    assert_eq!(
        saved["sleeping"]["slack"]["disabled"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        saved["sleeping"]["slack"]["alreadyDisabled"],
        json!(["xdg:user:slack-tray.desktop"])
    );
    assert_eq!(
        fs::read_to_string(&two).unwrap(),
        two_text,
        "already-disabled item untouched by sleep"
    );

    let r = call(&f, "optimizer.wake", json!({"appIds": ["slack"]})).unwrap();
    assert_eq!(r["results"][0]["ok"], true);
    assert_eq!(
        r["results"][0]["restoredItems"],
        json!(["xdg:user:slack.desktop"])
    );
    assert_eq!(fs::read_to_string(&one).unwrap(), SLACK, "byte-exact");
    assert_eq!(
        fs::read_to_string(&two).unwrap(),
        two_text,
        "stays disabled"
    );
    assert_eq!(state_json(&f)["sleeping"], json!({}));
    // waking something that is not asleep is reported, not fatal
    let r = call(&f, "optimizer.wake", json!({"appIds": ["slack"]})).unwrap();
    assert_eq!(r["results"][0]["ok"], false);
}

#[test]
fn wake_skips_items_that_vanished_and_keeps_going() {
    let f = fx(Os::Linux, vec![], true);
    let a = autostart(&f, "slack.desktop", SLACK);
    autostart(
        &f,
        "slack-2.desktop",
        "[Desktop Entry]\nName=Slack 2\nExec=/opt/Slack/slack --two\n",
    );
    call(&f, "optimizer.sleep", json!({"appIds": ["slack"]})).unwrap();
    fs::remove_file(&a).unwrap();
    let r = call(&f, "optimizer.wake", json!({"appIds": ["slack"]})).unwrap();
    let res = &r["results"][0];
    assert_eq!(res["missingItems"], json!(["Slack"]));
    assert_eq!(res["restoredItems"], json!(["xdg:user:slack-2.desktop"]));
    assert!(
        !fs::read_to_string(user_autostart_dir(&f.ctx).join("slack-2.desktop"))
            .unwrap()
            .contains("Hidden")
    );
}

#[test]
fn sleeping_twice_does_not_lose_the_original_record() {
    let f = fx(Os::Linux, slack_procs(), true);
    let path = autostart(&f, "slack.desktop", SLACK);
    call(&f, "optimizer.sleep", json!({"appIds": ["slack"]})).unwrap();
    call(&f, "optimizer.sleep", json!({"appIds": ["slack"]})).unwrap();
    call(&f, "optimizer.wake", json!({"appIds": ["slack"]})).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), SLACK);
}

#[test]
fn protected_and_unknown_apps_are_refused_without_changes() {
    let f = fx(Os::Linux, vec![], true);
    let text = "[Desktop Entry]\nName=Malwarebytes\nExec=/opt/malwarebytes/mbam\n";
    let p = autostart(&f, "malwarebytes.desktop", text);
    let r = call(
        &f,
        "optimizer.sleep",
        json!({"appIds": ["mbam", "does-not-exist"]}),
    )
    .unwrap();
    assert_eq!(r["results"][0]["ok"], false);
    assert!(r["results"][0]["error"]
        .as_str()
        .unwrap()
        .contains("protected"));
    assert_eq!(r["results"][1]["ok"], false);
    assert_eq!(fs::read_to_string(&p).unwrap(), text);
    assert!(!f.ctx.env.data_dir.join("optimizer.json").exists());
    assert_eq!(
        call(&f, "optimizer.sleep", json!({"appIds": []}))
            .unwrap_err()
            .code,
        ErrorCode::InvalidParams
    );
}

#[test]
fn processes_that_ignore_the_request_are_reported_never_killed() {
    set_quit_wait_ms(300);
    let f = fx(Os::Linux, slack_procs(), false);
    autostart(&f, "slack.desktop", SLACK);
    let started = std::time::Instant::now();
    let r = call(&f, "optimizer.sleep", json!({"appIds": ["slack"]})).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    let res = &r["results"][0];
    assert_eq!(res["stillRunning"], 3);
    assert_eq!(res["stoppedProcesses"], 0);
    assert!(res["note"].as_str().unwrap().contains("did not quit"));
    // only graceful requests were made: one per process
    assert_eq!(f.procs.exit_requests().len(), 3);
    assert_eq!(f.procs.list().len(), 3, "processes are still there");
}

#[test]
fn a_pid_that_now_belongs_to_another_process_is_not_signalled() {
    let f = fx(
        Os::Linux,
        vec![proc(100, "slack", "/opt/Slack/slack", 10)],
        true,
    );
    autostart(&f, "slack.desktop", SLACK);
    let a = analyze(&f.ctx, &job()).unwrap();
    let app = a.apps.iter().find(|x| x.id == "slack").unwrap().clone();
    // A different process source in which pid 100 is now "vim".
    let other = Arc::new(FakeProcesses::with_details(
        vec![proc(100, "vim", "/usr/bin/vim", 10)],
        true,
    ));
    let ctx2 = f.ctx.clone().with_procs(other.clone());
    let (gone, remaining, delivered) = stop_processes(&ctx2, &app, &job());
    assert_eq!((gone, remaining, delivered), (1, 0, 0));
    assert!(other.exit_requests().is_empty());
}

// ---------------------------------------------------------------- enforce

#[test]
fn enforce_redisables_reenabled_and_new_entries_and_never_touches_processes() {
    let f = fx(Os::Linux, slack_procs(), true);
    let path = autostart(&f, "slack.desktop", SLACK);
    call(&f, "optimizer.sleep", json!({"appIds": ["slack"]})).unwrap();
    assert_eq!(f.procs.exit_requests().len(), 3);

    // The user launched Slack again by hand: new processes, and Slack rewrote its autostart
    // file without the Hidden line and dropped a second entry.
    let relaunched = Arc::new(FakeProcesses::with_details(slack_procs(), true));
    let ctx = f.ctx.clone().with_procs(relaunched.clone());
    write(&path, SLACK);
    write(
        &user_autostart_dir(&ctx).join("slack-helper.desktop"),
        "[Desktop Entry]\nName=Slack helper\nExec=/opt/Slack/slack --helper\n",
    );
    let r = crate::api::dispatch(&ctx, "optimizer.enforce", json!({}), &job()).unwrap();
    let changed = r["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 2, "{r}");
    assert!(changed
        .iter()
        .any(|c| c["itemId"] == "xdg:user:slack.desktop" && c["reason"] == "re-enabled"));
    assert!(changed
        .iter()
        .any(|c| c["itemId"] == "xdg:user:slack-helper.desktop" && c["reason"] == "created"));
    assert!(fs::read_to_string(&path).unwrap().contains("Hidden=true"));
    assert!(
        relaunched.exit_requests().is_empty(),
        "enforce never signals a process"
    );
    assert_eq!(
        relaunched.list().len(),
        3,
        "user-launched processes keep running"
    );

    // Both are now recorded, so wake restores both.
    crate::api::dispatch(&ctx, "optimizer.wake", json!({"appIds": ["slack"]}), &job()).unwrap();
    assert!(!fs::read_to_string(&path).unwrap().contains("Hidden"));
    assert!(
        !fs::read_to_string(user_autostart_dir(&ctx).join("slack-helper.desktop"))
            .unwrap()
            .contains("Hidden")
    );

    // Nothing asleep: nothing to do.
    let r = crate::api::dispatch(&ctx, "optimizer.enforce", json!({}), &job()).unwrap();
    assert_eq!(r["changed"], json!([]));
}

#[test]
fn enforce_leaves_correctly_disabled_and_unrelated_entries_alone() {
    let f = fx(Os::Linux, vec![], true);
    autostart(&f, "slack.desktop", SLACK);
    let other = autostart(
        &f,
        "other.desktop",
        "[Desktop Entry]\nName=Other\nExec=/usr/bin/other\n",
    );
    call(&f, "optimizer.sleep", json!({"appIds": ["slack"]})).unwrap();
    let r = call(&f, "optimizer.enforce", json!({})).unwrap();
    assert_eq!(r["changed"], json!([]));
    assert!(!fs::read_to_string(other).unwrap().contains("Hidden"));
}

// ---------------------------------------------------------------- platforms

#[test]
fn windows_uses_taskkill_without_force() {
    let f = fx(
        Os::Windows,
        vec![
            proc(
                50,
                "Spotify.exe",
                r"C:\Users\u\AppData\Roaming\Spotify\Spotify.exe",
                200,
            ),
            proc(51, "svchost.exe", r"C:\Windows\System32\svchost.exe", 20),
            proc(52, "notepad.exe", r"C:\Windows\System32\notepad.exe", 5),
        ],
        true,
    );
    f.runner.on_any_args("taskkill", CmdOutput::ok("SUCCESS"));
    f.runner.on_any_args("schtasks", CmdOutput::failed(1, ""));
    f.runner.on_any_args("powershell", CmdOutput::failed(1, ""));
    let v = call(&f, "optimizer.analyze", json!({})).unwrap();
    assert_eq!(
        app_ids(&v),
        vec!["spotify"],
        "system processes are not apps"
    );
    let r = call(&f, "optimizer.sleep", json!({"appIds": ["spotify"]})).unwrap();
    assert_eq!(r["results"][0]["ok"], true);
    let kills: Vec<_> = f
        .runner
        .calls()
        .into_iter()
        .filter(|(p, _)| p == "taskkill")
        .collect();
    assert_eq!(kills.len(), 1);
    assert_eq!(kills[0].1, vec!["/PID", "50"]);
    assert!(!kills[0].1.iter().any(|a| a.eq_ignore_ascii_case("/f")));
    assert!(f.procs.exit_requests().is_empty(), "no signals on Windows");
}

#[test]
fn macos_quits_via_osascript_then_asks_the_os() {
    let f = fx(
        Os::MacOs,
        vec![
            proc(70, "Slack", "/Applications/Slack.app/Contents/MacOS/Slack", 300),
            proc(
                71,
                "Slack Helper",
                "/Applications/Slack.app/Contents/Frameworks/Slack Helper.app/Contents/MacOS/Slack Helper",
                100,
            ),
            proc(72, "Finder", "/System/Library/CoreServices/Finder.app/Contents/MacOS/Finder", 100),
            proc(73, "Dock", "/System/Library/CoreServices/Dock.app/Contents/MacOS/Dock", 100),
        ],
        false,
    );
    set_quit_wait_ms(300);
    f.runner.on_any_args("osascript", CmdOutput::ok(""));
    f.runner.on_any_args("launchctl", CmdOutput::failed(1, ""));
    f.runner.on_any_args("id", CmdOutput::ok("501"));
    let v = call(&f, "optimizer.analyze", json!({})).unwrap();
    assert_eq!(
        app_ids(&v),
        vec!["slack"],
        "helpers fold into the outer bundle; Finder and Dock are the session"
    );
    assert_eq!(app(&v, "slack")["processes"].as_array().unwrap().len(), 2);
    call(&f, "optimizer.sleep", json!({"appIds": ["slack"]})).unwrap();
    let quits: Vec<_> = f
        .runner
        .calls()
        .into_iter()
        .filter(|(p, a)| p == "osascript" && a[1] == "tell application \"Slack\" to quit")
        .collect();
    assert_eq!(quits.len(), 1);
    let mut req = f.procs.exit_requests();
    req.sort();
    assert_eq!(
        req,
        vec![70, 71],
        "SIGTERM (never SIGKILL) for what did not quit"
    );
}

#[test]
fn macos_launch_agent_sleep_does_not_bootout() {
    let f = fx(Os::MacOs, vec![], true);
    let dir = f.ctx.env.home.join("Library/LaunchAgents");
    fs::create_dir_all(&dir).unwrap();
    let mut d = plist::Dictionary::new();
    d.insert(
        "Label".into(),
        plist::Value::String("com.dropbox.agent".into()),
    );
    d.insert(
        "ProgramArguments".into(),
        plist::Value::Array(vec![plist::Value::String(
            "/Applications/Dropbox.app/Contents/MacOS/Dropbox".into(),
        )]),
    );
    plist::Value::Dictionary(d)
        .to_file_xml(dir.join("com.dropbox.agent.plist"))
        .unwrap();
    f.runner.on("id", &["-u"], CmdOutput::ok("501"));
    f.runner
        .on_any_args("launchctl", CmdOutput::ok("disabled services = {\n}\n"));
    f.runner.on_any_args("osascript", CmdOutput::ok(""));
    let r = call(&f, "optimizer.sleep", json!({"appIds": ["dropbox"]})).unwrap();
    assert_eq!(r["results"][0]["ok"], true, "{r}");
    let verbs: Vec<String> = f
        .runner
        .calls()
        .into_iter()
        .filter(|(p, _)| p == "launchctl")
        .map(|(_, a)| a[0].clone())
        .collect();
    assert!(verbs.contains(&"disable".to_string()));
    assert!(!verbs.contains(&"bootout".to_string()), "{verbs:?}");
}

// ---------------------------------------------------------------- state file

#[test]
fn a_damaged_state_file_is_reported_and_never_silently_reset() {
    let f = fx(Os::Linux, vec![], true);
    autostart(&f, "slack.desktop", SLACK);
    write(&f.ctx.env.data_dir.join("optimizer.json"), "{ not json");
    let v = call(&f, "optimizer.analyze", json!({})).unwrap();
    assert!(v["stateError"].as_str().unwrap().contains("damaged"));
    assert!(call(&f, "optimizer.sleep", json!({"appIds": ["slack"]})).is_err());
    assert!(call(&f, "optimizer.wake", json!({"appIds": ["slack"]})).is_err());
    assert_eq!(
        fs::read_to_string(f.ctx.env.data_dir.join("optimizer.json")).unwrap(),
        "{ not json"
    );
    assert!(
        !fs::read_to_string(user_autostart_dir(&f.ctx).join("slack.desktop"))
            .unwrap()
            .contains("Hidden")
    );
}

#[test]
fn cancellation_stops_before_changing_anything() {
    let f = fx(Os::Linux, vec![], true);
    let p = autostart(&f, "slack.desktop", SLACK);
    let j = Job::detached();
    j.token().cancel();
    let e = sleep_handler(&f.ctx, json!({"appIds": ["slack"]}), &j).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
    assert_eq!(fs::read_to_string(p).unwrap(), SLACK);
}

#[test]
fn security_software_matching() {
    for n in [
        "MsMpEng.exe",
        "clamd",
        "avast",
        "mbam",
        "Kaspersky",
        "falcon-sensor",
        "eset",
        "ekrn",
    ] {
        assert!(apps::is_security_software(n), "{n}");
    }
    for n in ["slack", "presets", "firefox", "assetmanager"] {
        assert!(!apps::is_security_software(n), "{n}");
    }
}
