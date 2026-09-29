use super::*;
use crate::api::dispatch;
use crate::procs::FakeProcesses;
use crate::runner::MockRunner;
use crate::testutil::{tree_snapshot, Chromium, Fixture};
use rusqlite::Connection;
use serde_json::json;
use std::sync::Arc;

struct Bed {
    _tmp: tempfile::TempDir,
    fx: Fixture,
    ctx: Ctx,
}

fn bed_with(procs: FakeProcesses) -> Bed {
    let tmp = tempfile::tempdir().unwrap();
    let fx = Fixture::new(tmp.path());
    let ctx = Ctx::new(fx.env.clone(), Arc::new(MockRunner::new())).with_procs(Arc::new(procs));
    Bed { _tmp: tmp, fx, ctx }
}

fn bed() -> Bed {
    bed_with(FakeProcesses::new(&[], false))
}

fn call(b: &Bed, m: &str, p: Value) -> Result<Value> {
    dispatch(&b.ctx, m, p, &Job::detached())
}

fn cookie_hosts(db: &std::path::Path) -> Vec<String> {
    let c = Connection::open(db).unwrap();
    let mut s = c
        .prepare("SELECT host_key FROM cookies ORDER BY host_key")
        .unwrap();
    s.query_map([], |r| r.get(0))
        .unwrap()
        .map(|x| x.unwrap())
        .collect()
}

#[test]
fn normalize_domain_cases() {
    for (input, want) in [
        ("Google.com", Some("google.com")),
        (".google.com", Some("google.com")),
        ("  *.Example.ORG ", Some("example.org")),
        ("https://www.Example.com/path?q=1", Some("www.example.com")),
        ("example.com:8080", Some("example.com")),
        ("example.com.", Some("example.com")),
        ("192.168.1.1", Some("192.168.1.1")),
        ("localhost", Some("localhost")),
        ("", None),
        (".", None),
        ("a b.com", None),
        ("bad/host", Some("bad")),
        ("a..b", None),
        ("we%ird.com", None),
        ("*", None),
        ("[::1]", None),
    ] {
        assert_eq!(normalize_domain(input).as_deref(), want, "{input:?}");
    }
}

#[test]
fn list_aggregates_across_browsers_and_profiles() {
    let b = bed();
    b.fx.populate_chromium(Chromium::Chrome, "Default");
    b.fx.populate_chromium(Chromium::Chrome, "Profile 1");
    b.fx.populate_chromium(Chromium::Edge, "Default");
    b.fx.populate_firefox("abcd.default-release");
    call(
        &b,
        "cookies.set_keep_list",
        json!({"domains": ["google.com"]}),
    )
    .unwrap();

    let v = call(&b, "cookies.list", Value::Null).unwrap();
    let list: Vec<CookieDomain> = serde_json::from_value(v).unwrap();
    let get = |d: &str| {
        list.iter()
            .find(|x| x.domain == d)
            .unwrap_or_else(|| panic!("{d}"))
    };

    // ".google.com" and "google.com" normalize to the same domain: 2 profiles x 2 + edge x 2
    let g = get("google.com");
    assert_eq!(g.count, 3 * 2);
    assert_eq!(g.browsers, vec!["Google Chrome", "Microsoft Edge"]);
    assert!(g.kept);
    assert!(
        get("accounts.google.com").kept,
        "subdomains of a kept domain are kept"
    );
    assert!(!get("notgoogle.com").kept);
    assert_eq!(get("tracker.example").count, 6);
    assert_eq!(get("mozilla.org").browsers, vec!["Mozilla Firefox"]);
    assert!(!get("mozilla.org").kept);
    // sorted, no leading dots, no empty domains
    assert!(list.windows(2).all(|w| w[0].domain < w[1].domain));
    assert!(list
        .iter()
        .all(|d| !d.domain.starts_with('.') && !d.domain.is_empty()));
    // camelCase wire shape
    let raw = call(&b, "cookies.list", Value::Null).unwrap();
    assert!(raw[0].get("browsers").is_some() && raw[0].get("kept").is_some());
}

#[test]
fn list_is_read_only() {
    let b = bed();
    b.fx.populate_chromium(Chromium::Chrome, "Default");
    b.fx.populate_firefox("abcd.default-release");
    let before = tree_snapshot(b._tmp.path());
    call(&b, "cookies.list", Value::Null).unwrap();
    call(&b, "cookies.get_keep_list", Value::Null).unwrap();
    assert_eq!(before, tree_snapshot(b._tmp.path()));
}

#[test]
fn keep_list_roundtrip_normalizes_dedupes_and_validates() {
    let b = bed();
    assert_eq!(
        call(&b, "cookies.get_keep_list", Value::Null).unwrap()["domains"],
        json!([])
    );
    let v = call(
        &b,
        "cookies.set_keep_list",
        json!({"domains": [" .GitHub.com", "github.com", "https://Example.org/x", "Zoom.us"]}),
    )
    .unwrap();
    assert_eq!(
        v["domains"],
        json!(["github.com", "example.org", "zoom.us"])
    );
    assert_eq!(
        call(&b, "cookies.get_keep_list", Value::Null).unwrap()["domains"],
        json!(["github.com", "example.org", "zoom.us"])
    );
    assert_eq!(
        settings::load(&b.ctx).cookie_keep,
        vec!["github.com", "example.org", "zoom.us"]
    );
    assert!(call(
        &b,
        "cookies.set_keep_list",
        json!({"domains": ["bad domain"]})
    )
    .is_err());
    assert!(call(&b, "cookies.set_keep_list", json!({})).is_err());
    // failed call left the list alone
    assert_eq!(settings::load(&b.ctx).cookie_keep.len(), 3);
    assert_eq!(
        call(&b, "cookies.set_keep_list", json!({"domains": []})).unwrap()["domains"],
        json!([])
    );
}

#[test]
fn intelligent_scan_adds_known_sites_found_in_cookie_databases() {
    let b = bed();
    b.fx.populate_chromium(Chromium::Chrome, "Default"); // google.com, github.com, notgoogle.com, tracker
    b.fx.populate_firefox("abcd.default-release"); // mozilla.org (not a known site)
    call(
        &b,
        "cookies.set_keep_list",
        json!({"domains": ["example.org"]}),
    )
    .unwrap();
    let v = call(&b, "cookies.intelligent_scan", Value::Null).unwrap();
    let added: Vec<&str> = v["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert!(added.contains(&"google.com") && added.contains(&"github.com"));
    assert!(!added.contains(&"notgoogle.com") && !added.contains(&"tracker.example"));
    let domains: Vec<&str> = v["domains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(domains[0], "example.org", "existing entries are kept first");
    assert!(domains.contains(&"google.com"));
    assert_eq!(
        settings::load(&b.ctx).cookie_keep,
        v["domains"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    );
    // idempotent
    let again = call(&b, "cookies.intelligent_scan", Value::Null).unwrap();
    assert_eq!(again["added"], json!([]));
    assert_eq!(again["domains"], v["domains"]);
}

#[test]
fn intelligent_scan_does_not_match_lookalike_domains() {
    let b = bed();
    let p = b.fx.chromium_profile(Chromium::Chrome, "Default");
    b.fx.sqlite(
        p.data.join("Network/Cookies"),
        "CREATE TABLE cookies(host_key TEXT, name TEXT);
         INSERT INTO cookies VALUES ('notgoogle.com','a'),('google.com.evil.io','b'),('mygithub.com','c');",
    );
    let v = call(&b, "cookies.intelligent_scan", Value::Null).unwrap();
    assert_eq!(v["added"], json!([]));
}

#[test]
fn known_sites_list_is_sane() {
    assert!(KNOWN_SITES.len() >= 60);
    let mut seen = HashSet::new();
    for s in KNOWN_SITES {
        assert_eq!(
            normalize_domain(s).as_deref(),
            Some(*s),
            "{s} must be normalized"
        );
        assert!(seen.insert(*s), "duplicate {s}");
    }
    for must in [
        "google.com",
        "github.com",
        "microsoft.com",
        "apple.com",
        "wikipedia.org",
        "atlassian.net",
    ] {
        assert!(KNOWN_SITES.contains(&must), "{must}");
    }
}

#[test]
fn delete_removes_domain_and_subdomains_in_every_browser() {
    let b = bed();
    let c1 = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let c2 = b.fx.populate_chromium(Chromium::Brave, "Default");
    let f = b.fx.populate_firefox("abcd.default-release");
    let v = call(
        &b,
        "cookies.delete",
        json!({"domains": ["Google.com", ".mozilla.org"]}),
    )
    .unwrap();
    // chrome: .google.com, accounts.google.com, google.com = 3; brave 3; firefox 2
    assert_eq!(v["deleted"], 3 + 3 + 2);
    let by: std::collections::HashMap<String, u64> = v["browsers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            (
                x["browser"].as_str().unwrap().to_string(),
                x["deleted"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(by["Google Chrome"], 3);
    assert_eq!(by["Brave"], 3);
    assert_eq!(by["Mozilla Firefox"], 2);
    let left = cookie_hosts(&c1.data.join("Network/Cookies"));
    assert_eq!(
        left,
        vec![
            ".github.com",
            ".tracker.example",
            ".tracker.example",
            "notgoogle.com"
        ]
    );
    assert_eq!(cookie_hosts(&c2.data.join("Network/Cookies")).len(), 4);
    let c = Connection::open(f.data.join("cookies.sqlite")).unwrap();
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM moz_cookies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 3);
    let lookalike: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM moz_cookies WHERE host='notmozilla.org'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lookalike, 1);
}

#[test]
fn delete_validates_input() {
    let b = bed();
    assert!(call(&b, "cookies.delete", json!({"domains": []})).is_err());
    assert!(call(&b, "cookies.delete", json!({"domains": ["bad domain"]})).is_err());
    assert!(call(&b, "cookies.delete", json!({})).is_err());
}

#[test]
fn delete_respects_running_browsers_and_locked_databases() {
    let b = bed_with(FakeProcesses::new(&["chrome"], false));
    let c = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let e = b.fx.populate_chromium(Chromium::Edge, "Default");
    let v = call(
        &b,
        "cookies.delete",
        json!({"domains": ["google.com"], "closeApps": "skip"}),
    )
    .unwrap();
    let chrome = v["browsers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["browser"] == "Google Chrome")
        .unwrap();
    assert_eq!(chrome["skipped"], "app_running");
    assert_eq!(chrome["deleted"], 0);
    assert_eq!(
        cookie_hosts(&c.data.join("Network/Cookies")).len(),
        7,
        "running browser untouched"
    );
    assert_eq!(
        cookie_hosts(&e.data.join("Network/Cookies")).len(),
        4,
        "other browser cleaned"
    );
    assert_eq!(v["deleted"], 3);

    // a locked database is reported, not forced
    let b = bed();
    let c = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let db = c.data.join("Network/Cookies");
    let holder = Connection::open(&db).unwrap();
    holder.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let v = call(&b, "cookies.delete", json!({"domains": ["google.com"]})).unwrap();
    assert_eq!(v["deleted"], 0);
    assert_eq!(v["browsers"][0]["skipped"], "in_use");
    holder.execute_batch("ROLLBACK").unwrap();
    drop(holder);
    assert_eq!(cookie_hosts(&db).len(), 7);
}

#[test]
fn delete_can_close_the_browser_first() {
    let procs = Arc::new(FakeProcesses::new(&["chrome"], true));
    let tmp = tempfile::tempdir().unwrap();
    let fx = Fixture::new(tmp.path());
    let ctx = Ctx::new(fx.env.clone(), Arc::new(MockRunner::new())).with_procs(procs.clone());
    let c = fx.populate_chromium(Chromium::Chrome, "Default");
    let v = dispatch(
        &ctx,
        "cookies.delete",
        json!({"domains": ["google.com"], "closeApps": "always"}),
        &Job::detached(),
    )
    .unwrap();
    assert_eq!(v["deleted"], 3);
    assert_eq!(procs.exit_requests().len(), 1);
    assert_eq!(cookie_hosts(&c.data.join("Network/Cookies")).len(), 4);
}

#[test]
fn delete_honours_the_exclusion_list() {
    let b = bed();
    let c = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let db = c.data.join("Network/Cookies");
    settings::update(&b.ctx, |s| {
        s.exclude.push(settings::ExcludeEntry {
            id: "x".into(),
            pattern: db.to_string_lossy().into_owned(),
        })
    })
    .unwrap();
    let v = call(&b, "cookies.delete", json!({"domains": ["google.com"]})).unwrap();
    assert_eq!(v["deleted"], 0);
    assert_eq!(cookie_hosts(&db).len(), 7);
}
