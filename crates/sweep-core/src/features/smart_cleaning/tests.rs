//! smart_cleaning API: config validation, junk check (+ auto clean) and agent status.

use super::*;
use crate::api::dispatch;
use crate::error::ErrorCode;
use crate::runner::MockRunner;
use crate::testutil::{Chromium, Fixture};
use serde_json::json;
use std::fs;
use std::sync::Arc;

const MB: usize = 1024 * 1024;

struct Bed {
    _tmp: tempfile::TempDir,
    fx: Fixture,
    ctx: Ctx,
}

fn bed() -> Bed {
    let tmp = tempfile::tempdir().unwrap();
    let fx = Fixture::new(tmp.path());
    let ctx = Ctx::new(fx.env.clone(), Arc::new(MockRunner::new()))
        .with_procs(Arc::new(crate::procs::FakeProcesses::new(&[], false)));
    Bed { _tmp: tmp, fx, ctx }
}

fn call(b: &Bed, m: &str, p: Value) -> Result<Value> {
    dispatch(&b.ctx, m, p, &Job::detached())
}

#[test]
fn get_config_returns_the_defaults() {
    let b = bed();
    assert_eq!(
        call(&b, "smart_cleaning.get_config", Value::Null).unwrap(),
        json!({
            "enabled": false,
            "thresholdMb": 500,
            "cleanOnBrowserClose": [],
            "autoClean": false,
            "notify": true,
            "checkIntervalMinutes": 60,
            "enforceSleepMinutes": 15
        })
    );
}

#[test]
fn set_config_merges_validates_and_persists() {
    let b = bed();
    let v = call(
        &b,
        "smart_cleaning.set_config",
        json!({"enabled": true, "thresholdMb": 1000, "cleanOnBrowserClose": ["Google Chrome", "Google Chrome"], "checkIntervalMinutes": 5}),
    )
    .unwrap();
    assert_eq!(v["enabled"], true);
    assert_eq!(v["thresholdMb"], 1000);
    assert_eq!(v["cleanOnBrowserClose"], json!(["Google Chrome"]));
    assert_eq!(v["notify"], true, "untouched fields keep their value");
    assert_eq!(v["checkIntervalMinutes"], 5);
    // It is the same data the settings API sees.
    let s = call(&b, "settings.get", Value::Null).unwrap();
    assert_eq!(s["smart"], v);
    // A second partial update keeps the first.
    let v2 = call(&b, "smart_cleaning.set_config", json!({"autoClean": true})).unwrap();
    assert_eq!(v2["thresholdMb"], 1000);
    assert_eq!(v2["autoClean"], true);
}

#[test]
fn set_config_rejects_bad_input_and_saves_nothing() {
    let b = bed();
    let bad = [
        (json!({"thresholdMb": 0}), "thresholdMb"),
        (
            json!({"thresholdMb": -5}),
            "invalid smart cleaning settings",
        ),
        (json!({"checkIntervalMinutes": 4}), "checkIntervalMinutes"),
        (
            json!({"checkIntervalMinutes": 10081}),
            "checkIntervalMinutes",
        ),
        (json!({"enforceSleepMinutes": 0}), "enforceSleepMinutes"),
        (json!({"enabled": "yes"}), "invalid smart cleaning settings"),
        (json!({"nope": 1}), "unknown smart cleaning setting `nope`"),
        (
            json!({"cleanOnBrowserClose": ["Netscape"]}),
            "not a browser",
        ),
        (json!({"cleanOnBrowserClose": [""]}), "not a browser"),
        (json!({"cleanOnBrowserClose": [5]}), "not a browser"),
    ];
    for (p, needle) in bad {
        let e = call(&b, "smart_cleaning.set_config", p.clone()).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams, "{p}");
        assert!(e.message.contains(needle), "{p}: {}", e.message);
    }
    assert!(call(&b, "smart_cleaning.set_config", json!([1])).is_err());
    assert!(!b.ctx.env.data_dir.join("settings.json").exists());
}

#[test]
fn browser_groups_are_the_browsers_with_rules_here() {
    let b = bed();
    let g = browser_groups(&b.ctx, &settings::load(&b.ctx));
    for want in [
        "Google Chrome",
        "Mozilla Firefox",
        "Microsoft Edge",
        "Brave",
    ] {
        assert!(g.contains(&want.to_string()), "{g:?}");
    }
    assert!(!g.contains(&"Custom".to_string()));
    assert!(!g.iter().any(|x| x == "Steam"), "apps are not browsers");
}

#[test]
fn the_new_settings_survive_old_and_damaged_files() {
    let b = bed();
    fs::create_dir_all(&b.ctx.env.data_dir).unwrap();
    // An old file without the new fields.
    fs::write(
        b.ctx.env.data_dir.join("settings.json"),
        r#"{"smart":{"enabled":true,"thresholdMb":800}}"#,
    )
    .unwrap();
    let s = settings::load(&b.ctx).smart;
    assert_eq!((s.enabled, s.threshold_mb), (true, 800));
    assert_eq!(
        (s.check_interval_minutes, s.enforce_sleep_minutes),
        (60, 15)
    );
    // A hand-edited out-of-range interval falls back to the defaults for the group.
    fs::write(
        b.ctx.env.data_dir.join("settings.json"),
        r#"{"smart":{"enabled":true,"checkIntervalMinutes":1}}"#,
    )
    .unwrap();
    assert_eq!(settings::load(&b.ctx).smart, SmartSettings::default());
}

fn junk_bed(mb: usize) -> Bed {
    let b = bed();
    let p = b.fx.chromium_profile(Chromium::Chrome, "Default");
    b.fx.file(p.cache.join("Cache/Cache_Data/big"), mb * MB);
    settings::update(&b.ctx, |s| {
        s.selected_rules = Some(vec!["chrome.cache".into()]);
        s.smart.threshold_mb = 1;
    })
    .unwrap();
    b
}

fn big_file(b: &Bed) -> std::path::PathBuf {
    b.fx.chromium_profile(Chromium::Chrome, "Default")
        .cache
        .join("Cache/Cache_Data/big")
}

#[test]
fn check_measures_the_enabled_rules_without_touching_anything() {
    let b = junk_bed(3);
    let v = call(&b, "smart_cleaning.check", Value::Null).unwrap();
    assert_eq!(v["junkBytes"], 3 * MB);
    assert_eq!(v["thresholdBytes"], MB);
    assert_eq!(v["overThreshold"], true);
    assert!(v.get("cleaned").is_none());
    assert!(big_file(&b).exists());

    settings::update(&b.ctx, |s| s.smart.threshold_mb = 4).unwrap();
    let v = call(&b, "smart_cleaning.check", Value::Null).unwrap();
    assert_eq!(v["overThreshold"], false);
    // Exactly at the threshold counts as over.
    settings::update(&b.ctx, |s| s.smart.threshold_mb = 3).unwrap();
    assert_eq!(
        call(&b, "smart_cleaning.check", Value::Null).unwrap()["overThreshold"],
        true
    );
}

#[test]
fn check_auto_cleans_only_when_enabled_and_over_the_threshold() {
    let b = junk_bed(3);
    let big = big_file(&b);
    // autoClean without smart cleaning enabled: never.
    settings::update(&b.ctx, |s| s.smart.auto_clean = true).unwrap();
    assert!(call(&b, "smart_cleaning.check", Value::Null)
        .unwrap()
        .get("cleaned")
        .is_none());
    assert!(big.exists());
    // Enabled but under the threshold: no clean.
    settings::update(&b.ctx, |s| {
        s.smart.enabled = true;
        s.smart.threshold_mb = 50;
    })
    .unwrap();
    assert!(call(&b, "smart_cleaning.check", Value::Null)
        .unwrap()
        .get("cleaned")
        .is_none());
    assert!(big.exists());
    // Over: cleaned, and the history says "smart".
    settings::update(&b.ctx, |s| s.smart.threshold_mb = 1).unwrap();
    let v = call(&b, "smart_cleaning.check", Value::Null).unwrap();
    assert_eq!(v["cleaned"]["removedBytes"], 3 * MB);
    assert_eq!(v["cleaned"]["removedFiles"], 1);
    assert!(v["cleaned"]["historyId"].is_string());
    assert!(!big.exists());
    let h = call(&b, "cleaner.history", json!({"limit": 1})).unwrap();
    assert_eq!(h[0]["source"], "smart");
    assert_eq!(h[0]["id"], v["cleaned"]["historyId"]);
    // Nothing left: the next check is under the threshold.
    let v = call(&b, "smart_cleaning.check", Value::Null).unwrap();
    assert_eq!(v["junkBytes"], 0);
    assert_eq!(v["overThreshold"], false);
}

#[test]
fn status_reports_the_agent_through_its_lock() {
    let b = bed();
    let v = call(&b, "smart_cleaning.status", Value::Null).unwrap();
    assert_eq!(v, json!({"running": false}));
    let lock = crate::agent::AgentLock::try_acquire(&b.ctx)
        .unwrap()
        .unwrap();
    let v = call(&b, "smart_cleaning.status", Value::Null).unwrap();
    assert_eq!(v["running"], true);
    drop(lock);
    assert_eq!(
        call(&b, "smart_cleaning.status", Value::Null).unwrap()["running"],
        false
    );
}
