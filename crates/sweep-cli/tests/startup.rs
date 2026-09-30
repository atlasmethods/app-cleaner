//! Real-binary checks for the startup manager, optimizer and browser plugins against a
//! throwaway HOME (no fakes for the file system; the PATH is empty so no external tool
//! such as `systemctl` or `crontab` is ever run).

use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::Command;

fn call(base: &Path, method: &str, params: Value) -> (bool, Value) {
    let home = base.join("home");
    let empty = base.join("empty-path");
    fs::create_dir_all(&empty).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_clearsweep"))
        .args(["call", method, &params.to_string()])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("CLEARSWEEP_ROOT", base.join("root"))
        .env("CLEARSWEEP_DATA_DIR", base.join("data"))
        .env("TMPDIR", base.join("root/tmp"))
        .env("PATH", &empty)
        .env("CLEARSWEEP_FAKE_PROCESSES", "")
        .env("CLEARSWEEP_TEST_IGNORE_CTIME", "1")
        .output()
        .unwrap();
    let text = if out.status.success() {
        &out.stdout
    } else {
        &out.stderr
    };
    (
        out.status.success(),
        serde_json::from_slice(text).unwrap_or(Value::Null),
    )
}

const SLACK: &str = "[Desktop Entry]\nType=Application\nName=Slack\nExec=/opt/Slack/slack -u %U\n";

#[test]
fn real_xdg_autostart_toggle_in_a_temp_home() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("home/.config/autostart/slack.desktop");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, SLACK).unwrap();

    let (ok, list) = call(d.path(), "startup.list", json!({}));
    assert!(ok, "{list}");
    let item = list
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "xdg:user:slack.desktop")
        .expect("listed");
    assert_eq!(item["enabled"], true);
    assert_eq!(item["name"], "Slack");

    let (ok, r) = call(
        d.path(),
        "startup.set_enabled",
        json!({"id": "xdg:user:slack.desktop", "enabled": false}),
    );
    assert!(ok, "{r}");
    assert_eq!(r["item"]["enabled"], false);
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        format!("{SLACK}Hidden=true\n")
    );

    let (ok, r) = call(
        d.path(),
        "startup.set_enabled",
        json!({"id": "xdg:user:slack.desktop", "enabled": true}),
    );
    assert!(ok, "{r}");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        SLACK,
        "byte-exact after re-enabling"
    );

    // remove -> backup -> restore
    let (ok, r) = call(
        d.path(),
        "startup.remove",
        json!({"id": "xdg:user:slack.desktop"}),
    );
    assert!(ok, "{r}");
    assert!(!file.exists());
    let backup = r["backupId"].as_str().unwrap().to_string();
    assert!(d
        .path()
        .join("data/backups")
        .join(&backup)
        .join("manifest.json")
        .is_file());
    let (ok, r) = call(d.path(), "startup.restore_backup", json!({"id": backup}));
    assert!(ok, "{r}");
    assert_eq!(fs::read_to_string(&file).unwrap(), SLACK);
}

#[test]
fn real_optimizer_sleep_and_wake_round_trip() {
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("home/.config/autostart/slack.desktop");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, SLACK).unwrap();
    let (ok, v) = call(d.path(), "optimizer.analyze", json!({}));
    assert!(ok, "{v}");
    let app = v["apps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["appId"] == "slack")
        .unwrap();
    assert_eq!(app["startupIds"], json!(["xdg:user:slack.desktop"]));
    let (ok, v) = call(d.path(), "optimizer.sleep", json!({"appIds": ["slack"]}));
    assert!(ok, "{v}");
    assert!(fs::read_to_string(&file).unwrap().contains("Hidden=true"));
    let (ok, v) = call(d.path(), "optimizer.enforce", json!({}));
    assert!(ok && v["changed"] == json!([]), "{v}");
    let (ok, v) = call(d.path(), "optimizer.wake", json!({"appIds": ["slack"]}));
    assert!(ok, "{v}");
    assert_eq!(fs::read_to_string(&file).unwrap(), SLACK);
}

#[test]
fn real_plugins_list_reads_a_profile_on_disk() {
    let d = tempfile::tempdir().unwrap();
    let profile = d
        .path()
        .join("home/.config/BraveSoftware/Brave-Browser/Default");
    let ext = "abcdefghijklmnopabcdefghijklmnop";
    fs::create_dir_all(profile.join(format!("Extensions/{ext}/1.0_0"))).unwrap();
    fs::write(
        profile.join(format!("Extensions/{ext}/1.0_0/manifest.json")),
        r#"{"name":"Brave Helper","version":"1.0"}"#,
    )
    .unwrap();
    fs::write(
        profile.join("Preferences"),
        json!({"extensions": {"settings": {ext: {"location": 1, "path": format!("{ext}/1.0_0"), "state": 1}}}}).to_string(),
    )
    .unwrap();
    let (ok, v) = call(d.path(), "browser_plugins.list", json!({}));
    assert!(ok, "{v}");
    let p = &v.as_array().unwrap()[0];
    assert_eq!(p["name"], "Brave Helper");
    assert_eq!(p["browser"], "brave");
    assert_eq!(p["canDisable"], true);
    let (ok, r) = call(
        d.path(),
        "browser_plugins.set_enabled",
        json!({"id": p["id"], "enabled": false}),
    );
    assert!(ok, "{r}");
    let prefs: Value =
        serde_json::from_str(&fs::read_to_string(profile.join("Preferences")).unwrap()).unwrap();
    assert_eq!(prefs["extensions"]["settings"][ext]["state"], 0);
}
