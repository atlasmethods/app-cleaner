use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_clearsweep"))
}

#[test]
fn call_sysinfo_prints_json() {
    let out = bin().args(["call", "sysinfo.get"]).output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["cpu"]["cores"].as_u64().unwrap() >= 1);
}

#[test]
fn call_unknown_method_exits_1_with_error_json() {
    let out = bin().args(["call", "nope.nothing"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(v["code"], "NotFound");
}

#[test]
fn call_bad_params_json_exits_1() {
    let out = bin()
        .args(["call", "api.methods", "{oops"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(v["code"], "InvalidParams");
}

use std::path::Path;
use sweep_core::testutil::{query_i64, Chromium, Fixture};

/// A command whose every path (HOME, XDG_*, temp, system root, data dir) is inside `base`
/// and that sees a fixed list of running processes.
fn sandboxed(base: &Path, fake_procs: &str) -> Command {
    let home = base.join("home");
    let mut c = bin();
    c.env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("CLEARSWEEP_ROOT", base.join("root"))
        .env("CLEARSWEEP_DATA_DIR", base.join("data"))
        .env("TMPDIR", base.join("root/tmp"))
        .env("CLEARSWEEP_TEST_IGNORE_CTIME", "1")
        .env("CLEARSWEEP_FAKE_PROCESSES", fake_procs);
    c
}

fn sandbox() -> (tempfile::TempDir, Fixture) {
    let d = tempfile::tempdir().unwrap();
    let fx = Fixture::new(d.path());
    (d, fx)
}

#[test]
fn analyze_json_reports_sizes_and_changes_nothing() {
    let (d, fx) = sandbox();
    let p = fx.populate_chromium(Chromium::Chrome, "Default");
    let out = sandboxed(d.path(), "")
        .args([
            "analyze",
            "--rules",
            "chrome.cache,chrome.history",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    assert_eq!(v["items"][0]["ruleId"], "chrome.cache");
    assert_eq!(v["items"][0]["bytes"], 100 * 1024 + 4096);
    assert_eq!(v["items"][1]["rows"], 7);
    assert_eq!(v["totalFiles"], 5);
    assert!(p.cache.join("Cache/Cache_Data/data_0").exists());
}

#[test]
fn analyze_prints_a_human_table() {
    let (d, fx) = sandbox();
    fx.populate_chromium(Chromium::Chrome, "Default");
    let out = sandboxed(d.path(), "")
        .args(["analyze", "--rules", "chrome.cache,edge.cache"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("Google Chrome - Internet Cache"), "{text}");
    assert!(text.contains("104.0 KB"), "{text}");
    assert!(
        !text.contains("Microsoft Edge"),
        "empty rules are hidden: {text}"
    );
    assert!(text.contains("Total: 104.0 KB in 5 files"), "{text}");
}

#[test]
fn clean_auto_removes_files_and_records_history_with_the_source() {
    let (d, fx) = sandbox();
    let p = fx.populate_chromium(Chromium::Chrome, "Default");
    let out = sandboxed(d.path(), "")
        .args(["clean", "--auto", "--rules", "chrome.cache"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("chrome.cache"), "{text}");
    assert!(text.contains("removed 104.0 KB in 5 files"), "{text}");
    assert!(!p.cache.join("Cache/Cache_Data/data_0").exists());
    assert!(p.data.join("Bookmarks").exists());
    let h: serde_json::Value =
        serde_json::from_slice(&std::fs::read(d.path().join("data/history.json")).unwrap())
            .unwrap();
    assert_eq!(h[0]["source"], "auto");

    let out = sandboxed(d.path(), "")
        .args([
            "clean",
            "--auto",
            "--rules",
            "chrome.history",
            "--source",
            "scheduled",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["totalRows"], 7);
    let h: serde_json::Value =
        serde_json::from_slice(&std::fs::read(d.path().join("data/history.json")).unwrap())
            .unwrap();
    assert_eq!(h.as_array().unwrap().len(), 2);
    assert_eq!(h[1]["source"], "scheduled");
}

#[test]
fn clean_auto_uses_saved_selection_and_treats_ask_as_skip() {
    let (d, fx) = sandbox();
    let p = fx.populate_chromium(Chromium::Chrome, "Default");
    std::fs::create_dir_all(d.path().join("data")).unwrap();
    std::fs::write(
        d.path().join("data/settings.json"),
        r#"{"selectedRules":["chrome.cache","chrome.session"],"closeBrowsers":"ask"}"#,
    )
    .unwrap();
    // Chrome is "running": with ask == skip nothing may be touched, exit code is still 0.
    let out = sandboxed(d.path(), "chrome")
        .args(["clean", "--auto"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("skipped (app running: Google Chrome)"),
        "{text}"
    );
    assert!(p.cache.join("Cache/Cache_Data/data_0").exists());
    assert!(p.data.join("Current Session").exists());
    // Not running: the saved selection (cache + session, nothing else) is cleaned.
    let out = sandboxed(d.path(), "")
        .args(["clean", "--auto"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(!p.cache.join("Cache/Cache_Data/data_0").exists());
    assert!(!p.data.join("Current Session").exists());
    assert!(p.data.join("History").exists());
    assert_eq!(
        query_i64(&p.data.join("History"), "SELECT COUNT(*) FROM urls"),
        3,
        "history rule was not selected"
    );
}

#[test]
fn clean_without_auto_is_refused_with_exit_2() {
    let (d, _fx) = sandbox();
    let out = sandboxed(d.path(), "").arg("clean").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(v["code"], "InvalidParams");
}

#[test]
fn unknown_rule_exits_1_and_deletes_nothing() {
    let (d, fx) = sandbox();
    let p = fx.populate_chromium(Chromium::Chrome, "Default");
    for args in [
        &["clean", "--auto", "--rules", "chrome.cache,nope.nothing"][..],
        &["analyze", "--rules", "nope.nothing"][..],
    ] {
        let out = sandboxed(d.path(), "").args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let v: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(v["code"], "InvalidParams");
    }
    assert!(p.cache.join("Cache/Cache_Data/data_0").exists());
}

#[cfg(feature = "testutil")]
#[test]
fn dev_fixture_builds_a_machine_and_refuses_other_directories() {
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join("clearsweep-test-fixture");
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/settings.json"), "{}").unwrap();
    let out = bin().arg("dev-fixture").arg(&dir).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.join("home").exists() && dir.join("root").exists());
    assert!(
        !dir.join("data/settings.json").exists(),
        "data dir is reset"
    );
    // idempotent
    assert!(bin()
        .arg("dev-fixture")
        .arg(&dir)
        .output()
        .unwrap()
        .status
        .success());
    // guard
    let other = base.path().join("precious");
    std::fs::create_dir_all(other.join("home")).unwrap();
    let out = bin().arg("dev-fixture").arg(&other).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(other.join("home").exists());
}
