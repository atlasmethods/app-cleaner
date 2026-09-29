//! Behavioural tests of the cleaner against realistic fixture trees.

use super::*;
use crate::api::dispatch;
use crate::ctx::{Env, Os};
use crate::procs::FakeProcesses;
use crate::runner::{CmdOutput, MockRunner};
use crate::testutil::{tree_snapshot, Chromium, Fixture};
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

struct Bed {
    _tmp: tempfile::TempDir,
    fx: Fixture,
    ctx: Ctx,
    mock: MockRunner,
}

fn bed_for(os: Os) -> Bed {
    let tmp = tempfile::tempdir().unwrap();
    let mut fx = Fixture::new(tmp.path());
    fx.env.os = os;
    let mock = MockRunner::new();
    let procs = Arc::new(FakeProcesses::new(&[], false));
    let ctx = Ctx::new(fx.env.clone(), Arc::new(mock.clone())).with_procs(procs);
    Bed {
        _tmp: tmp,
        fx,
        ctx,
        mock,
    }
}

fn bed() -> Bed {
    bed_for(Os::current())
}

impl Bed {
    fn base(&self) -> &Path {
        self._tmp.path()
    }
    fn set(&self, f: impl FnOnce(&mut Settings)) {
        settings::update(&self.ctx, f).unwrap();
    }
    fn analyze(&self, ids: &[&str]) -> AnalyzeReport {
        analyze(
            &self.ctx,
            Some(ids.iter().map(|s| s.to_string()).collect()),
            &Job::detached(),
        )
        .unwrap()
    }
    fn item(&self, id: &str) -> AnalyzeItem {
        self.analyze(&[id]).items.remove(0)
    }
    fn clean(&self, ids: &[&str]) -> CleanReport {
        self.clean_with(ids, CloseBrowsers::Skip)
    }
    fn clean_with(&self, ids: &[&str], close: CloseBrowsers) -> CleanReport {
        let mut opts = CleanOptions::new(close);
        opts.close_timeout = Duration::from_millis(300);
        run_clean(
            &self.ctx,
            Some(ids.iter().map(|s| s.to_string()).collect()),
            &opts,
            Source::Manual,
            &Job::detached(),
        )
        .unwrap()
    }
    fn result<'a>(&self, r: &'a CleanReport, id: &str) -> &'a RuleClean {
        r.results.iter().find(|x| x.rule_id == id).unwrap()
    }
}

fn q1(db: &Path, sql: &str) -> i64 {
    Connection::open(db)
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

fn all_rule_ids(b: &Bed) -> Vec<String> {
    available_rules(&b.ctx, &settings::load(&b.ctx))
        .into_iter()
        .map(|r| r.id)
        .collect()
}

// ------------------------------------------------------------------ analysis

#[test]
fn analyze_never_modifies_anything() {
    let b = bed();
    b.fx.populate_typical();
    b.set(|s| {
        s.include.push(settings::IncludeEntry {
            id: "i".into(),
            path: b.fx.env.home.join("scratch").to_string_lossy().into_owned(),
            recursive: true,
            mask: "*".into(),
            remove_empty_dirs: true,
        });
        s.cookie_keep = vec!["google.com".into()];
    });
    b.fx.file(b.fx.env.home.join("scratch/a.txt"), 100);
    let before = tree_snapshot(b.base());
    let ids = all_rule_ids(&b);
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let rep = b.analyze(&ids);
    assert!(rep.total_files > 0 && rep.total_rows > 0);
    let after = tree_snapshot(b.base());
    assert_eq!(before, after, "analysis changed the tree");
}

#[test]
fn analyze_reports_chrome_cache_size_exactly() {
    let b = bed();
    b.fx.populate_chromium(Chromium::Chrome, "Default");
    let it = b.item("chrome.cache");
    assert_eq!(it.bytes, Fixture::chromium_cache_bytes() + 4096);
    assert_eq!(it.files, 5);
    assert!(it.sample_paths.len() == 5);
    assert!(!it.app_running);
    assert!(it.errors.is_empty(), "{:?}", it.errors);
    // a browser that is not installed reports nothing
    let e = b.item("edge.cache");
    assert_eq!((e.files, e.bytes, e.rows), (0, 0, 0));
}

#[test]
fn unknown_rule_id_and_client_paths_are_rejected() {
    let b = bed();
    let e = analyze(&b.ctx, Some(vec!["nope.nothing".into()]), &Job::detached()).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    for m in ["cleaner.clean", "cleaner.analyze"] {
        let e = dispatch(
            &b.ctx,
            m,
            json!({"ruleIds": ["chrome.cache"], "paths": ["/etc"]}),
            &Job::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams, "{m}");
    }
    // rules of another OS are not selectable
    let other = if cfg!(windows) {
        "linux.temp"
    } else {
        "windows.temp"
    };
    assert!(analyze(&b.ctx, Some(vec![other.into()]), &Job::detached()).is_err());
}

#[test]
fn analyze_cancellation_returns_cancelled() {
    let b = bed();
    b.fx.populate_typical();
    let job = Job::detached();
    job.token().cancel();
    let e = analyze(&b.ctx, None, &job).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
}

// ------------------------------------------------------------------ clean == analyze

#[test]
fn clean_removes_exactly_what_analyze_reported() {
    let b = bed();
    b.fx.populate_typical();
    b.fx.file(b.fx.env.home.join("Documents/thesis.docx"), 12345);
    let ids = [
        "chrome.cache",
        "chrome.history",
        "chrome.downloads",
        "chrome.autofill",
        "edge.cache",
        "brave.history",
        "firefox.cache",
        "firefox.history",
        "firefox.formhistory",
        "firefox.session",
        "linux.temp",
        "linux.trash",
        "linux.thumbnails",
        "linux.recent",
        "linux.apt",
        "linux.logs",
    ];
    let report = b.analyze(&ids);
    let before = tree_snapshot(b.base());
    let cleaned = b.clean(&ids);
    let after = tree_snapshot(b.base());
    assert_eq!(cleaned.results.len(), ids.len());

    let mut expected_removed: Vec<String> = Vec::new();
    for item in &report.items {
        let r = b.result(&cleaned, &item.rule_id);
        assert_eq!(r.skipped, None, "{}", item.rule_id);
        assert_eq!(r.removed_files, item.files, "files of {}", item.rule_id);
        assert_eq!(r.removed_bytes, item.bytes, "bytes of {}", item.rule_id);
        assert_eq!(r.removed_rows, item.rows, "rows of {}", item.rule_id);
        assert!(r.failed.is_empty(), "{}: {:?}", item.rule_id, r.failed);
        assert!(item.sample_paths.len() < 50);
        for p in &item.sample_paths {
            if !p.ends_with(".sqlite") && !p.ends_with("History") && !p.ends_with("Web Data") {
                expected_removed.push(p.clone());
            }
        }
    }
    assert_eq!(cleaned.total_files, report.total_files);
    assert_eq!(cleaned.total_bytes, report.total_bytes);
    assert_eq!(cleaned.total_rows, report.total_rows);
    assert!(report.total_files > 10);

    // Every file the analysis named is gone...
    let base = b.base().to_string_lossy().into_owned();
    for p in &expected_removed {
        assert!(!Path::new(p).exists(), "{p} should be gone");
    }
    // ...and every other *file* is byte-for-byte unchanged (databases are edited in
    // place, so only their rows change - checked in the dedicated tests below).
    for (rel, desc) in &before {
        let full = format!("{base}/{rel}");
        if expected_removed.contains(&full) || !desc.starts_with("file") {
            continue;
        }
        if rel.ends_with("History") || rel.ends_with(".sqlite") || rel.ends_with("Web Data") {
            continue;
        }
        assert_eq!(after.get(rel), Some(desc), "{rel} must be untouched");
    }
    // Sensitive neighbours are intact.
    assert!(b.fx.env.home.join("Documents/thesis.docx").exists());
    let chrome = b.fx.chromium_profile(Chromium::Chrome, "Default");
    assert!(chrome.data.join("Bookmarks").exists());
    assert!(chrome.data.join("Preferences").exists());
    assert!(chrome.data.join("Network/Cookies").exists());
    assert!(chrome.data.join("Login Data").exists());
    // History was recorded.
    assert_eq!(history::list(&b.ctx, None).len(), 1);
}

#[test]
fn second_clean_finds_nothing_left_of_the_same_rules() {
    let b = bed();
    b.fx.populate_typical();
    let ids = [
        "chrome.cache",
        "linux.temp",
        "firefox.history",
        "chrome.history",
    ];
    b.clean(&ids);
    let again = b.analyze(&ids);
    assert_eq!(again.total_files, 0);
    assert_eq!(again.total_rows, 0);
}

// ------------------------------------------------------------------ browsers

#[test]
fn firefox_bookmarks_survive_history_cleaning() {
    let b = bed();
    let p = b.fx.populate_firefox("abcd.default-release");
    let places = p.data.join("places.sqlite");
    let r = b.clean(&["firefox.history"]);
    let rc = b.result(&r, "firefox.history");
    assert_eq!(rc.skipped, None, "{:?}", rc.failed);
    assert_eq!(rc.removed_rows, 4); // moz_historyvisits rows
    assert_eq!(q1(&places, "SELECT COUNT(*) FROM moz_historyvisits"), 0);
    assert_eq!(q1(&places, "SELECT COUNT(*) FROM moz_inputhistory"), 0);
    // bookmark + the page it points to survive; unreferenced pages are gone
    assert_eq!(q1(&places, "SELECT COUNT(*) FROM moz_bookmarks"), 2);
    assert_eq!(
        q1(
            &places,
            "SELECT COUNT(*) FROM moz_places WHERE url='https://bookmarked.example/'"
        ),
        1
    );
    assert_eq!(q1(&places, "SELECT COUNT(*) FROM moz_places"), 1);
    assert_eq!(
        q1(&places, "SELECT visit_count FROM moz_places WHERE id=3"),
        0
    );
    // orphaned origins were pruned, the bookmarked one stays
    assert_eq!(q1(&places, "SELECT COUNT(*) FROM moz_origins"), 1);
    // download history and bookmark notes are separate rules/data and are untouched
    assert_eq!(q1(&places, "SELECT COUNT(*) FROM moz_annos"), 3);
    let ic: String = Connection::open(&places)
        .unwrap()
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ic, "ok");
}

#[test]
fn firefox_download_history_only_removes_download_annotations() {
    let b = bed();
    let p = b.fx.populate_firefox("abcd.default-release");
    let places = p.data.join("places.sqlite");
    assert_eq!(b.item("firefox.downloads").rows, 2);
    let r = b.clean(&["firefox.downloads"]);
    assert_eq!(b.result(&r, "firefox.downloads").removed_rows, 2);
    assert_eq!(q1(&places, "SELECT COUNT(*) FROM moz_annos"), 1);
    assert_eq!(
        q1(
            &places,
            "SELECT COUNT(*) FROM moz_annos WHERE content='keep this note'"
        ),
        1
    );
    assert_eq!(q1(&places, "SELECT COUNT(*) FROM moz_places"), 3);
    assert_eq!(q1(&places, "SELECT COUNT(*) FROM moz_historyvisits"), 4);
}

#[test]
fn chrome_downloads_survive_history_but_not_download_history_cleaning() {
    let b = bed();
    let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let hist = p.data.join("History");

    let r = b.clean(&["chrome.history"]);
    assert_eq!(b.result(&r, "chrome.history").removed_rows, 3 + 4);
    assert_eq!(q1(&hist, "SELECT COUNT(*) FROM urls"), 0);
    assert_eq!(q1(&hist, "SELECT COUNT(*) FROM visits"), 0);
    assert_eq!(q1(&hist, "SELECT COUNT(*) FROM keyword_search_terms"), 0);
    assert_eq!(q1(&hist, "SELECT COUNT(*) FROM segments"), 0);
    assert_eq!(q1(&hist, "SELECT COUNT(*) FROM segment_usage"), 0);
    assert_eq!(q1(&hist, "SELECT COUNT(*) FROM visit_source"), 0);
    assert_eq!(
        q1(&hist, "SELECT COUNT(*) FROM downloads"),
        2,
        "downloads must survive"
    );
    assert_eq!(q1(&hist, "SELECT COUNT(*) FROM downloads_url_chains"), 2);

    let r = b.clean(&["chrome.downloads"]);
    assert_eq!(b.result(&r, "chrome.downloads").removed_rows, 2);
    assert_eq!(q1(&hist, "SELECT COUNT(*) FROM downloads"), 0);
    assert_eq!(q1(&hist, "SELECT COUNT(*) FROM downloads_url_chains"), 0);
}

#[test]
fn chrome_download_cleaning_keeps_browsing_history() {
    let b = bed();
    let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
    b.clean(&["chrome.downloads"]);
    assert_eq!(q1(&p.data.join("History"), "SELECT COUNT(*) FROM urls"), 3);
    assert_eq!(
        q1(&p.data.join("History"), "SELECT COUNT(*) FROM visits"),
        4
    );
}

#[test]
fn chrome_autofill_and_passwords() {
    let b = bed();
    let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
    b.clean(&["chrome.autofill"]);
    let wd = p.data.join("Web Data");
    assert_eq!(q1(&wd, "SELECT COUNT(*) FROM autofill"), 0);
    assert_eq!(
        q1(&wd, "SELECT COUNT(*) FROM autofill_profiles"),
        1,
        "addresses stay"
    );
    let r = b.clean(&["chrome.passwords"]);
    assert_eq!(b.result(&r, "chrome.passwords").removed_rows, 2);
    assert_eq!(
        q1(&p.data.join("Login Data"), "SELECT COUNT(*) FROM logins"),
        0
    );
}

#[test]
fn chrome_session_removes_session_files_only() {
    let b = bed();
    let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let r = b.clean(&["chrome.session"]);
    let rc = b.result(&r, "chrome.session");
    assert_eq!(rc.removed_files, 3);
    assert_eq!(rc.removed_bytes, 2048 + 1024 + 512);
    assert!(!p.data.join("Sessions/Session_13300000000").exists());
    assert!(!p.data.join("Current Session").exists());
    assert!(p.data.join("Bookmarks").exists());
    assert!(p.data.join("Preferences").exists());
}

#[test]
fn firefox_session_form_history_and_passwords() {
    let b = bed();
    let p = b.fx.populate_firefox("abcd.default-release");
    let r = b.clean(&["firefox.session"]);
    assert_eq!(b.result(&r, "firefox.session").removed_files, 3);
    assert!(!p.data.join("sessionstore.jsonlz4").exists());
    assert!(!p
        .data
        .join("sessionstore-backups/recovery.jsonlz4")
        .exists());
    assert!(p.data.join("prefs.js").exists());
    b.clean(&["firefox.formhistory"]);
    assert_eq!(
        q1(
            &p.data.join("formhistory.sqlite"),
            "SELECT COUNT(*) FROM moz_formhistory"
        ),
        0
    );
    let r = b.clean(&["firefox.passwords"]);
    assert_eq!(b.result(&r, "firefox.passwords").removed_files, 2);
    assert!(!p.data.join("logins.json").exists());
    assert!(!p.data.join("key4.db").exists());
    assert!(p.data.join("prefs.js").exists());
}

#[test]
fn several_chromium_profiles_and_browsers_are_all_cleaned() {
    let b = bed();
    b.fx.populate_chromium(Chromium::Chrome, "Default");
    b.fx.populate_chromium(Chromium::Chrome, "Profile 1");
    b.fx.populate_chromium(Chromium::Brave, "Default");
    let it = b.item("chrome.cache");
    assert_eq!(it.bytes, 2 * (Fixture::chromium_cache_bytes() + 4096));
    let h = b.item("chrome.history");
    assert_eq!(h.rows, 2 * 7);
    b.clean(&["chrome.cache", "chrome.history"]);
    assert_eq!(b.item("chrome.cache").files, 0);
    // Brave was not selected: untouched
    assert_eq!(b.item("brave.cache").files, 5);
}

#[test]
fn firefox_profile_from_profiles_ini_outside_the_default_folder() {
    let b = bed();
    b.fx.populate_firefox("abcd.default-release");
    // a second profile in a custom location, only known through profiles.ini
    let custom = b.fx.env.home.join("ff-custom");
    let places = b.fx.sqlite(
        custom.join("places.sqlite"),
        "CREATE TABLE moz_historyvisits(id INTEGER PRIMARY KEY, place_id INTEGER);
         INSERT INTO moz_historyvisits VALUES (1,1),(2,1),(3,1);",
    );
    let ini = b.fx.firefox_ini_path();
    let mut text = fs::read_to_string(&ini).unwrap();
    text.push_str(&format!(
        "\n[Profile1]\nName=custom\nIsRelative=0\nPath={}\n",
        custom.display()
    ));
    fs::write(&ini, text).unwrap();
    assert_eq!(b.item("firefox.history").rows, 4 + 3);
    b.clean(&["firefox.history"]);
    assert_eq!(q1(&places, "SELECT COUNT(*) FROM moz_historyvisits"), 0);
}

// ------------------------------------------------------------------ cookies

fn hosts(db: &Path, table: &str, col: &str) -> Vec<String> {
    let c = Connection::open(db).unwrap();
    let mut s = c
        .prepare(&format!("SELECT {col} FROM {table} ORDER BY {col}"))
        .unwrap();
    s.query_map([], |r| r.get(0))
        .unwrap()
        .map(|x| x.unwrap())
        .collect()
}

#[test]
fn keep_list_cookies_survive_including_subdomains_but_not_lookalikes() {
    let b = bed();
    let c = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let f = b.fx.populate_firefox("abcd.default-release");
    b.set(|s| s.cookie_keep = vec!["Google.com".into(), ".mozilla.org".into()]);
    assert_eq!(b.item("chrome.cookies").rows, 4); // notgoogle, tracker x2, github
    assert_eq!(b.item("firefox.cookies").rows, 3); // ads x2, notmozilla
    let r = b.clean(&["chrome.cookies", "firefox.cookies"]);
    assert_eq!(b.result(&r, "chrome.cookies").removed_rows, 4);
    assert_eq!(b.result(&r, "firefox.cookies").removed_rows, 3);
    assert_eq!(
        hosts(&c.data.join("Network/Cookies"), "cookies", "host_key"),
        vec![".google.com", "accounts.google.com", "google.com"]
    );
    assert_eq!(
        hosts(&f.data.join("cookies.sqlite"), "moz_cookies", "host"),
        vec![".mozilla.org", "accounts.mozilla.org"]
    );
}

#[test]
fn empty_keep_list_removes_every_cookie() {
    let b = bed();
    let c = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let r = b.clean(&["chrome.cookies"]);
    assert_eq!(b.result(&r, "chrome.cookies").removed_rows, 7);
    assert!(hosts(&c.data.join("Network/Cookies"), "cookies", "host_key").is_empty());
}

#[test]
fn chromium_cookies_in_the_legacy_location_are_cleaned_too() {
    let b = bed();
    let c = b.fx.chromium_profile(Chromium::Chrome, "Default");
    let legacy = b.fx.sqlite(
        c.data.join("Cookies"),
        "CREATE TABLE cookies(host_key TEXT, name TEXT, value TEXT);
         INSERT INTO cookies VALUES ('.a.com','x','1'),('keep.org','y','2');",
    );
    b.set(|s| s.cookie_keep = vec!["keep.org".into()]);
    let r = b.clean(&["chrome.cookies"]);
    assert_eq!(b.result(&r, "chrome.cookies").removed_rows, 1);
    assert_eq!(hosts(&legacy, "cookies", "host_key"), vec!["keep.org"]);
}

// ------------------------------------------------------------------ symlinks

#[cfg(unix)]
mod symlinks {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn symlink_inside_cache_dir_is_removed_and_its_target_left_alone() {
        let b = bed();
        let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
        let outside_file = b.fx.file(b.fx.env.home.join("precious/file.txt"), 999);
        let outside_dir = b.fx.dir(b.fx.env.home.join("precious/dir"));
        b.fx.file(outside_dir.join("inner.txt"), 10);
        let cache = p.cache.join("Cache/Cache_Data");
        symlink(&outside_file, cache.join("link-to-file")).unwrap();
        symlink(&outside_dir, cache.join("link-to-dir")).unwrap();
        let before_outside = tree_snapshot(&b.fx.env.home.join("precious"));

        let it = b.item("chrome.cache");
        assert_eq!(it.files, 7, "links count as removable entries");
        assert_eq!(
            it.bytes,
            Fixture::chromium_cache_bytes() + 4096,
            "links weigh 0 bytes"
        );
        let r = b.clean(&["chrome.cache"]);
        assert_eq!(b.result(&r, "chrome.cache").removed_files, 7);
        assert!(fs::symlink_metadata(cache.join("link-to-file")).is_err());
        assert!(fs::symlink_metadata(cache.join("link-to-dir")).is_err());
        assert_eq!(
            tree_snapshot(&b.fx.env.home.join("precious")),
            before_outside
        );
    }

    #[test]
    fn symlinked_cache_directory_is_never_followed() {
        let b = bed();
        let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
        let outside = b.fx.dir(b.fx.env.home.join("elsewhere"));
        b.fx.file(outside.join("important.dat"), 2000);
        // replace "Code Cache" by a symlink to an outside directory
        fs::remove_dir_all(p.cache.join("Code Cache")).unwrap();
        symlink(&outside, p.cache.join("Code Cache")).unwrap();

        let it = b.item("chrome.cache");
        assert_eq!(it.bytes, 60 * 1024 + 4096 + 15 * 1024, "Code Cache skipped");
        assert!(
            it.errors
                .iter()
                .any(|e| e.message.contains("symbolic link")),
            "{:?}",
            it.errors
        );
        b.clean(&["chrome.cache"]);
        assert!(outside.join("important.dat").exists());
        assert!(
            fs::symlink_metadata(p.cache.join("Code Cache")).is_ok(),
            "the link itself is left"
        );
    }

    #[test]
    fn symlinked_profile_directory_is_ignored() {
        let b = bed();
        let outside = b.fx.dir(b.fx.env.home.join("other-profile"));
        b.fx.file(outside.join("Cache/data"), 4000);
        let root = b.fx.chromium_profile(Chromium::Chrome, "Default").cache;
        b.fx.dir(root.parent().unwrap());
        symlink(&outside, root.parent().unwrap().join("Profile 9")).unwrap();
        // Windows/mac keep cache next to data; the glob only follows real directories
        assert_eq!(b.item("chrome.cache").files, 0);
        b.clean(&["chrome.cache"]);
        assert!(outside.join("Cache/data").exists());
    }

    #[test]
    fn symlinked_database_is_skipped() {
        let b = bed();
        let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
        let real = b.fx.home_copy(&p.data.join("History"), "history-copy");
        fs::remove_file(p.data.join("History")).unwrap();
        symlink(&real, p.data.join("History")).unwrap();
        let r = b.clean(&["chrome.history"]);
        let rc = b.result(&r, "chrome.history");
        assert_eq!(rc.removed_rows, 0);
        assert!(rc
            .failed
            .iter()
            .any(|f| f.message.contains("symbolic link")));
        assert_eq!(q1(&real, "SELECT COUNT(*) FROM urls"), 3);
    }

    #[test]
    fn sockets_and_fifos_in_temp_are_never_touched() {
        let b = bed();
        let sock = b.fx.env.temp_dir.join("app.sock2");
        let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let fifo = b.fx.env.temp_dir.join("pipe1");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        crate::testutil::set_age_hours(&sock, 500);
        crate::testutil::set_age_hours(&fifo, 500);
        b.fx.aged_file(b.fx.env.temp_dir.join("old.tmp"), 10, 500);
        let r = b.clean(&["linux.temp"]);
        assert_eq!(b.result(&r, "linux.temp").removed_files, 1);
        assert!(fs::symlink_metadata(&sock).is_ok());
        assert!(fs::symlink_metadata(&fifo).is_ok());
    }
}

impl Fixture {
    /// Copy a file into the home directory (test helper).
    fn home_copy(&self, from: &Path, name: &str) -> PathBuf {
        let to = self.env.home.join(name);
        fs::copy(from, &to).unwrap();
        to
    }
}

// ------------------------------------------------------------------ exclusions

#[test]
fn excluded_file_and_directory_survive_and_are_not_counted() {
    let b = bed();
    let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let keep_file = p.cache.join("Cache/Cache_Data/keepme.bin");
    let keep_dir = p.cache.join("Code Cache/js");
    b.set(|s| {
        s.exclude = vec![
            settings::ExcludeEntry {
                id: "a".into(),
                pattern: keep_file.to_string_lossy().into_owned(),
            },
            settings::ExcludeEntry {
                id: "b".into(),
                pattern: keep_dir.to_string_lossy().into_owned(),
            },
        ]
    });
    let it = b.item("chrome.cache");
    assert_eq!(it.bytes, 60 * 1024 + 15 * 1024);
    assert_eq!(it.files, 3);
    assert!(it.errors.is_empty(), "{:?}", it.errors);
    let r = b.clean(&["chrome.cache"]);
    assert_eq!(b.result(&r, "chrome.cache").removed_files, 3);
    assert!(keep_file.exists());
    assert!(keep_dir.join("index").exists());
    assert!(!p.cache.join("Cache/Cache_Data/data_0").exists());
}

#[test]
fn excluding_a_database_skips_it() {
    let b = bed();
    let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
    b.set(|s| {
        s.exclude = vec![settings::ExcludeEntry {
            id: "a".into(),
            pattern: p.data.join("History").to_string_lossy().into_owned(),
        }]
    });
    assert_eq!(b.item("chrome.history").rows, 0);
    b.clean(&["chrome.history"]);
    assert_eq!(q1(&p.data.join("History"), "SELECT COUNT(*) FROM urls"), 3);
}

#[test]
fn glob_exclusion_pattern_with_tilde() {
    let b = bed();
    let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let home = b.fx.env.home.clone();
    let rel = p
        .cache
        .strip_prefix(&home)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    b.set(|s| {
        s.exclude = vec![settings::ExcludeEntry {
            id: "a".into(),
            pattern: format!("~/{rel}/Cache/Cache_Data/data_*"),
        }]
    });
    b.clean(&["chrome.cache"]);
    assert!(p.cache.join("Cache/Cache_Data/data_0").exists());
    assert!(p.cache.join("Cache/Cache_Data/data_1").exists());
    assert!(!p.cache.join("Cache/Cache_Data/keepme.bin").exists());
}

#[test]
fn invalid_exclusion_in_settings_aborts_cleaning_before_deleting_anything() {
    let b = bed();
    let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
    fs::create_dir_all(&b.fx.env.data_dir).unwrap();
    fs::write(
        b.fx.env.data_dir.join("settings.json"),
        r#"{"exclude":[{"id":"x","pattern":"relative/oops"}]}"#,
    )
    .unwrap();
    let e = run_clean(
        &b.ctx,
        Some(vec!["chrome.cache".into()]),
        &CleanOptions::new(CloseBrowsers::Skip),
        Source::Manual,
        &Job::detached(),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    assert!(p.cache.join("Cache/Cache_Data/data_0").exists());
    assert!(analyze(&b.ctx, None, &Job::detached()).is_err());
}

// ------------------------------------------------------------------ include entries

#[test]
fn include_entry_is_cleaned_with_its_mask_recursively() {
    let b = bed();
    let root = b.fx.env.home.join("scratch");
    b.fx.file(root.join("a.tmp"), 10);
    b.fx.file(root.join("keep.txt"), 20);
    b.fx.file(root.join("sub/b.tmp"), 30);
    b.fx.file(root.join("sub/keep2.txt"), 40);
    b.fx.file(root.join("emptyafter/c.tmp"), 50);
    let flat = b.fx.env.home.join("flat");
    b.fx.file(flat.join("x.tmp"), 5);
    b.fx.file(flat.join("deep/y.tmp"), 5);
    b.set(|s| {
        s.include = vec![
            settings::IncludeEntry {
                id: "1".into(),
                path: root.to_string_lossy().into_owned(),
                recursive: true,
                mask: "*.tmp".into(),
                remove_empty_dirs: true,
            },
            settings::IncludeEntry {
                id: "2".into(),
                path: "~/flat".into(),
                recursive: false,
                mask: "*.tmp".into(),
                remove_empty_dirs: false,
            },
        ]
    });
    let it = b.item(CUSTOM_RULE_ID);
    assert_eq!(it.files, 3 + 1);
    assert_eq!(it.bytes, 10 + 30 + 50 + 5);
    let r = b.clean(&[CUSTOM_RULE_ID]);
    let rc = b.result(&r, CUSTOM_RULE_ID);
    assert_eq!(rc.removed_files, 4);
    assert!(!root.join("a.tmp").exists());
    assert!(!root.join("sub/b.tmp").exists());
    assert!(root.join("keep.txt").exists());
    assert!(root.join("sub/keep2.txt").exists());
    assert!(!root.join("emptyafter").exists(), "emptied dir removed");
    assert!(root.exists(), "base is kept");
    assert!(!flat.join("x.tmp").exists());
    assert!(flat.join("deep/y.tmp").exists(), "not recursive");
}

#[test]
fn include_rule_only_exists_when_configured_and_is_in_the_custom_group() {
    let b = bed();
    let has = |b: &Bed| {
        list_rules(&b.ctx)
            .categories
            .iter()
            .flat_map(|c| c.groups.iter())
            .any(|g| g.group == "Custom" && g.rules.iter().any(|r| r.id == CUSTOM_RULE_ID))
    };
    assert!(!has(&b));
    b.set(|s| {
        s.include.push(settings::IncludeEntry {
            id: "1".into(),
            path: "~/scratch".into(),
            recursive: true,
            mask: "*".into(),
            remove_empty_dirs: false,
        })
    });
    assert!(has(&b));
}

#[test]
fn include_of_a_protected_folder_cannot_delete_it() {
    // The settings layer refuses the entry; if a hand-edited file slips one in, it is
    // dropped on load. Either way nothing in ~/Documents is touched.
    let b = bed();
    let docs = b.fx.env.home.join("Documents");
    b.fx.file(docs.join("a.txt"), 10);
    fs::create_dir_all(&b.fx.env.data_dir).unwrap();
    fs::write(
        b.fx.env.data_dir.join("settings.json"),
        format!(
            r#"{{"include":[{{"id":"x","path":"{}","recursive":true}},{{"id":"y","path":"{}","recursive":true}}]}}"#,
            docs.display(),
            b.fx.env.home.display()
        ),
    )
    .unwrap();
    assert!(custom_rule(&b.ctx, &settings::load(&b.ctx)).is_none());
    b.clean(&[]);
    assert!(docs.join("a.txt").exists());
}

#[test]
fn engine_refuses_a_protected_base_even_if_a_rule_names_it() {
    let b = bed();
    let docs = b.fx.env.home.join("Documents");
    b.fx.file(docs.join("a.txt"), 10);
    b.fx.file(b.fx.env.home.join("top.txt"), 10);
    let mk = |base: &Path| {
        let mut r = custom_rule(
            &b.ctx,
            &Settings {
                include: vec![settings::IncludeEntry {
                    id: "1".into(),
                    path: b.fx.env.home.join("zzz").to_string_lossy().into_owned(),
                    recursive: true,
                    mask: "*".into(),
                    remove_empty_dirs: true,
                }],
                ..Settings::default()
            },
        )
        .unwrap();
        if let Target::Files(f) = &mut r.targets[0] {
            f.literal_base = Some(base.to_path_buf());
        }
        r
    };
    let s = settings::load(&b.ctx);
    let safety = build_safety(&b.ctx, &s).unwrap();
    let eng = engine(&b.ctx, &s, safety, Mode::Clean);
    for base in [docs.clone(), b.fx.env.home.clone(), b.fx.env.root.clone()] {
        let mut out = Outcome::default();
        eng.run_rule(&mk(&base), &Job::detached(), &mut out)
            .unwrap();
        assert_eq!(out.files, 0, "{base:?}");
        assert_eq!(out.errors.len(), 1, "{base:?}: {:?}", out.errors);
        assert!(
            out.errors[0].message.contains("protected"),
            "{:?}",
            out.errors
        );
    }
    assert!(docs.join("a.txt").exists());
    assert!(b.fx.env.home.join("top.txt").exists());
}

// ------------------------------------------------------------------ temp files

#[cfg(unix)]
#[test]
fn temp_files_respect_minimum_age_locks_and_skip_names() {
    let b = bed();
    let sys = b.fx.populate_linux_system();
    let r = b.clean(&["linux.temp"]);
    let rc = b.result(&r, "linux.temp");
    assert!(rc.failed.is_empty(), "{:?}", rc.failed);
    assert!(!sys.old_tmp.exists());
    assert!(!sys.old_tmp_nested.exists());
    assert!(!sys.var_tmp_old.exists());
    assert!(sys.new_tmp.exists(), "younger than 24h survives");
    assert!(sys.lock_file.exists(), "lock files survive");
    assert!(sys.x11_socket_dir_file.exists(), ".X11-unix survives");
    assert_eq!(rc.removed_files, 3);
    // emptied directories are removed only when old enough; `build/cache` was old
    assert!(!b.fx.env.temp_dir.join("build").exists());
    assert!(b.fx.env.temp_dir.exists());
}

#[test]
fn temp_min_age_setting_is_honoured() {
    let b = bed();
    let sys = b.fx.populate_linux_system();
    b.set(|s| s.temp_min_age_hours = 200);
    assert_eq!(b.item("linux.temp").files, 0);
    b.set(|s| s.temp_min_age_hours = 0);
    let it = b.item("linux.temp");
    assert_eq!(
        it.files, 4,
        "new.tmp qualifies at age 0; locks/X11 still skipped"
    );
    b.clean(&["linux.temp"]);
    assert!(!sys.new_tmp.exists());
    assert!(sys.lock_file.exists());
}

#[cfg(unix)]
#[test]
fn new_empty_directories_in_temp_survive_but_old_ones_go() {
    let b = bed();
    let t = &b.fx.env.temp_dir;
    b.fx.dir(t.join("fresh-empty"));
    b.fx.dir(t.join("stale-empty"));
    crate::testutil::set_age_hours(&t.join("stale-empty"), 500);
    b.clean(&["linux.temp"]);
    assert!(t.join("fresh-empty").exists());
    assert!(!t.join("stale-empty").exists());
}

#[cfg(unix)]
#[test]
fn files_owned_by_other_users_are_not_touched() {
    if unsafe { libc::geteuid() } != 0 {
        return; // chown needs root
    }
    let b = bed();
    let mine = b.fx.aged_file(b.fx.env.temp_dir.join("mine.tmp"), 10, 500);
    let theirs =
        b.fx.aged_file(b.fx.env.temp_dir.join("theirs.tmp"), 10, 500);
    std::os::unix::fs::chown(&theirs, Some(12345), Some(12345)).unwrap();
    let it = b.item("linux.temp");
    assert_eq!(it.files, 1);
    b.clean(&["linux.temp"]);
    assert!(!mine.exists());
    assert!(theirs.exists());
}

// ------------------------------------------------------------------ linux system rules

#[test]
fn linux_trash_thumbnails_recent_apt_logs_crash() {
    let b = bed();
    let s = b.fx.populate_linux_system();
    let ids = [
        "linux.trash",
        "linux.thumbnails",
        "linux.recent",
        "linux.apt",
        "linux.logs",
        "linux.crash",
    ];
    let rep = b.analyze(&ids);
    let by = |id: &str| rep.items.iter().find(|i| i.rule_id == id).unwrap().clone();
    assert_eq!(by("linux.trash").files, 3);
    assert_eq!(by("linux.trash").bytes, 2500 + 100 + 400);
    assert_eq!(by("linux.apt").files, 1);
    assert_eq!(by("linux.logs").files, 3);
    b.clean(&ids);
    assert!(!s.trash_file.exists() && !s.trash_info.exists() && !s.trash_dir_file.exists());
    assert!(
        b.fx.env.user_data_dir.join("Trash/files").exists(),
        "Trash folders stay"
    );
    assert!(!b.fx.env.user_data_dir.join("Trash/files/folder").exists());
    assert!(!s.thumbnail.exists());
    assert!(!s.recent.exists());
    assert!(!s.apt_deb.exists());
    assert!(s.apt_partial.exists(), "not recursive: partial/ survives");
    assert!(s.apt_lock.exists());
    assert!(s.log_active.exists(), "the live log survives");
    assert!(!s.log_rot1.exists() && !s.log_gz.exists() && !s.log_old.exists());
    assert!(!s.crash.exists());
}

#[test]
fn clipboard_command_runs_only_when_the_program_exists() {
    let b = bed();
    // nothing installed: nothing to do
    let it = b.item("linux.clipboard");
    assert!(it.actions.is_empty());
    let r = b.clean(&["linux.clipboard"]);
    assert!(b.mock.calls().is_empty());
    assert_eq!(b.result(&r, "linux.clipboard").actions.len(), 0);

    b.mock.on(
        "xclip",
        &["-selection", "clipboard", "-i", "/dev/null"],
        CmdOutput::ok(""),
    );
    let it = b.item("linux.clipboard");
    assert_eq!(it.actions, vec!["Clear clipboard (X11, xclip)"]);
    assert!(b.mock.calls().is_empty(), "analysis never runs commands");
    let r = b.clean(&["linux.clipboard"]);
    assert_eq!(
        b.result(&r, "linux.clipboard").actions,
        vec!["Clear clipboard (X11, xclip)"]
    );
    assert_eq!(b.mock.calls().len(), 1);
    assert_eq!(b.mock.calls()[0].0, "xclip");
}

#[test]
fn failing_command_is_reported_not_fatal() {
    let b = bed();
    b.mock.on(
        "resolvectl",
        &["flush-caches"],
        CmdOutput::failed(1, "Failed to flush: access denied\nmore"),
    );
    let r = b.clean(&["linux.dns"]);
    let rc = b.result(&r, "linux.dns");
    assert!(rc.actions.is_empty());
    assert_eq!(rc.failed.len(), 1);
    assert_eq!(rc.failed[0].message, "Failed to flush: access denied");
}

// ------------------------------------------------------------------ in-use databases

#[test]
fn locked_database_is_skipped_as_in_use_without_corruption() {
    let b = bed();
    let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let hist = p.data.join("History");
    let holder = Connection::open(&hist).unwrap();
    holder.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let r = b.clean(&["chrome.history"]);
    let rc = b.result(&r, "chrome.history");
    assert_eq!(rc.skipped, Some(Skipped::InUse));
    assert_eq!(rc.removed_rows, 0);
    assert!(rc.failed.iter().any(|f| f.message == "in use"));
    holder.execute_batch("ROLLBACK").unwrap();
    drop(holder);
    assert_eq!(q1(&hist, "SELECT COUNT(*) FROM urls"), 3);
    let ic: String = Connection::open(&hist)
        .unwrap()
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ic, "ok");
    // not recorded as a run
    assert!(history::list(&b.ctx, None).is_empty());
    // and works once the lock is gone
    let r = b.clean(&["chrome.history"]);
    assert_eq!(b.result(&r, "chrome.history").skipped, None);
}

#[test]
fn one_locked_profile_does_not_block_the_other() {
    let b = bed();
    let p1 = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let p2 = b.fx.populate_chromium(Chromium::Chrome, "Profile 1");
    let holder = Connection::open(p1.data.join("History")).unwrap();
    holder.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let r = b.clean(&["chrome.history"]);
    let rc = b.result(&r, "chrome.history");
    assert_eq!(rc.skipped, None);
    assert_eq!(rc.removed_rows, 7);
    assert_eq!(rc.failed.len(), 1);
    assert_eq!(q1(&p2.data.join("History"), "SELECT COUNT(*) FROM urls"), 0);
    holder.execute_batch("ROLLBACK").unwrap();
    assert_eq!(q1(&p1.data.join("History"), "SELECT COUNT(*) FROM urls"), 3);
}

// ------------------------------------------------------------------ running apps

#[test]
fn running_app_marks_analysis_and_skips_the_clean_when_asking_or_skipping() {
    let b = bed();
    b.fx.populate_chromium(Chromium::Chrome, "Default");
    let procs = Arc::new(FakeProcesses::new(&["chrome", "bash"], false));
    let ctx = Ctx::new(b.fx.env.clone(), Arc::new(b.mock.clone())).with_procs(procs.clone());
    let rep = analyze(
        &ctx,
        Some(vec!["chrome.cache".into(), "edge.cache".into()]),
        &Job::detached(),
    )
    .unwrap();
    assert!(rep.items[0].app_running);
    assert!(!rep.items[1].app_running);
    for policy in [CloseBrowsers::Ask, CloseBrowsers::Skip] {
        let r = run_clean(
            &ctx,
            Some(vec!["chrome.cache".into()]),
            &CleanOptions::new(policy),
            Source::Manual,
            &Job::detached(),
        )
        .unwrap();
        assert_eq!(r.results[0].skipped, Some(Skipped::AppRunning));
        assert_eq!(r.results[0].running_apps, vec!["Google Chrome"]);
        assert_eq!(r.total_files, 0);
    }
    assert!(
        procs.exit_requests().is_empty(),
        "no closing without permission"
    );
    assert_eq!(b.item("chrome.cache").files, 5, "files are still there");
    assert!(history::list(&ctx, None).is_empty());
}

#[test]
fn close_apps_always_closes_gracefully_then_cleans() {
    let b = bed();
    b.fx.populate_chromium(Chromium::Chrome, "Default");
    let procs = Arc::new(FakeProcesses::new(&["chrome", "chrome"], true));
    let ctx = Ctx::new(b.fx.env.clone(), Arc::new(b.mock.clone())).with_procs(procs.clone());
    // terminate_closes only removes the process that received the request; the
    // second "chrome" also gets a request because all matches are signalled.
    let r = run_clean(
        &ctx,
        Some(vec!["chrome.cache".into(), "chrome.history".into()]),
        &CleanOptions::new(CloseBrowsers::Always),
        Source::Manual,
        &Job::detached(),
    )
    .unwrap();
    assert_eq!(procs.exit_requests().len(), 2);
    assert_eq!(r.results[0].skipped, None);
    assert_eq!(r.results[0].closed_apps, vec!["Google Chrome"]);
    assert_eq!(r.results[0].removed_files, 5);
    assert!(
        r.results[1].closed_apps.is_empty(),
        "already closed for the first rule"
    );
    assert_eq!(r.results[1].removed_rows, 7);
}

#[test]
fn app_that_refuses_to_close_is_skipped_never_force_killed() {
    let b = bed();
    b.fx.populate_chromium(Chromium::Chrome, "Default");
    let procs = Arc::new(FakeProcesses::new(&["chrome"], false));
    let ctx = Ctx::new(b.fx.env.clone(), Arc::new(b.mock.clone())).with_procs(procs.clone());
    let mut opts = CleanOptions::new(CloseBrowsers::Always);
    opts.close_timeout = Duration::from_millis(250);
    let started = std::time::Instant::now();
    let r = run_clean(
        &ctx,
        Some(vec!["chrome.cache".into()]),
        &opts,
        Source::Manual,
        &Job::detached(),
    )
    .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(250));
    assert_eq!(r.results[0].skipped, Some(Skipped::AppRunning));
    assert_eq!(procs.exit_requests().len(), 1);
    assert_eq!(b.item("chrome.cache").files, 5);
}

#[test]
fn windows_close_uses_taskkill_without_force_and_macos_uses_osascript() {
    for (os, program, args_have) in [
        (Os::Windows, "taskkill", vec!["/IM", "chrome.exe"]),
        (
            Os::MacOs,
            "osascript",
            vec!["-e", "tell application \"Google Chrome\" to quit"],
        ),
    ] {
        let b = bed_for(os);
        b.fx.populate_chromium(Chromium::Chrome, "Default");
        let procs = Arc::new(FakeProcesses::new(
            &[if os == Os::Windows {
                "chrome.exe"
            } else {
                "Google Chrome"
            }],
            false,
        ));
        let ctx = Ctx::new(b.fx.env.clone(), Arc::new(b.mock.clone())).with_procs(procs);
        let mut opts = CleanOptions::new(CloseBrowsers::Always);
        opts.close_timeout = Duration::from_millis(150);
        let r = run_clean(
            &ctx,
            Some(vec!["chrome.cache".into()]),
            &opts,
            Source::Manual,
            &Job::detached(),
        )
        .unwrap();
        assert_eq!(r.results[0].skipped, Some(Skipped::AppRunning));
        let calls = b.mock.calls();
        assert_eq!(calls.len(), 1, "{os:?}");
        assert_eq!(calls[0].0, program);
        assert_eq!(calls[0].1, args_have);
        assert!(!calls[0].1.iter().any(|a| a.eq_ignore_ascii_case("/f")));
    }
}

// ------------------------------------------------------------------ other operating systems (rule logic)

#[test]
fn windows_rules_resolve_localappdata_and_appdata() {
    let b = bed_for(Os::Windows);
    let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
    assert!(p.data.starts_with(&b.fx.env.data_local_dir));
    assert_eq!(b.item("chrome.cache").files, 5);
    assert_eq!(b.item("chrome.history").rows, 7);
    b.fx.aged_file(b.fx.env.temp_dir.join("old.tmp"), 100, 500);
    b.fx.aged_file(b.fx.env.sys_path("/Windows/Temp/x.tmp"), 100, 500);
    let it = b.item("windows.temp");
    assert_eq!(it.files, 2);
    // Recent documents live in %APPDATA%
    b.fx.file(
        b.fx.env.config_dir.join("Microsoft/Windows/Recent/doc.lnk"),
        10,
    );
    b.fx.file(
        b.fx.env
            .config_dir
            .join("Microsoft/Windows/Recent/AutomaticDestinations/x.automaticDestinations-ms"),
        10,
    );
    assert_eq!(b.item("windows.recentdocs").files, 1);
    assert_eq!(b.item("windows.jumplists").files, 1);
    b.clean(&["windows.recentdocs"]);
    assert!(b
        .fx
        .env
        .config_dir
        .join("Microsoft/Windows/Recent/AutomaticDestinations")
        .exists());
}

#[test]
fn windows_recycle_bin_uses_powershell_via_the_runner() {
    let b = bed_for(Os::Windows);
    // not installed: nothing runs, rule is unsupported
    let r = b.clean(&["windows.recyclebin"]);
    assert_eq!(
        b.result(&r, "windows.recyclebin").skipped,
        Some(Skipped::Unsupported)
    );

    let script_args = [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        engine::RECYCLE_SIZE_SCRIPT,
    ];
    b.mock
        .on("powershell", &script_args, CmdOutput::ok("3 1500\r\n"));
    b.mock.on(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            engine::RECYCLE_CLEAR_SCRIPT,
        ],
        CmdOutput::ok(""),
    );
    let it = b.item("windows.recyclebin");
    assert_eq!((it.files, it.bytes), (3, 1500));
    assert_eq!(b.mock.calls().len(), 1, "analysis only measures");
    assert!(!b.mock.calls()[0]
        .1
        .iter()
        .any(|a| a.contains("Clear-RecycleBin")));
    let r = b.clean(&["windows.recyclebin"]);
    let rc = b.result(&r, "windows.recyclebin");
    assert_eq!((rc.removed_files, rc.removed_bytes), (3, 1500));
    assert_eq!(rc.actions, vec!["Empty Recycle Bin"]);
}

#[test]
fn windows_recycle_bin_size_unknown_is_still_offered() {
    let b = bed_for(Os::Windows);
    b.mock.with_program("powershell");
    let it = b.item("windows.recyclebin");
    assert_eq!(it.files, 0);
    assert_eq!(it.actions, vec!["Empty Recycle Bin (size unknown)"]);
}

#[cfg(not(windows))]
#[test]
fn registry_rules_are_unsupported_when_not_on_windows() {
    let b = bed_for(Os::Windows);
    let r = b.clean(&["windows.explorer_mru"]);
    assert_eq!(
        b.result(&r, "windows.explorer_mru").skipped,
        Some(Skipped::Unsupported)
    );
}

#[test]
fn macos_caches_skip_apple_system_caches() {
    let b = bed_for(Os::MacOs);
    let c = &b.fx.env.cache_dir;
    b.fx.file(c.join("com.apple.Safari/x.db"), 100);
    b.fx.file(c.join("org.example.app/cache.bin"), 200);
    b.fx.file(c.join("Homebrew/dl.tgz"), 300);
    let it = b.item("macos.caches");
    assert_eq!(it.files, 2);
    assert_eq!(it.bytes, 500);
    b.clean(&["macos.caches"]);
    assert!(c.join("com.apple.Safari/x.db").exists());
    assert!(!c.join("org.example.app/cache.bin").exists());
    assert!(c.exists());
}

#[test]
fn macos_trash_and_logs() {
    let b = bed_for(Os::MacOs);
    b.fx.file(b.fx.env.home.join(".Trash/old.dmg"), 1000);
    b.fx.file(b.fx.env.home.join("Library/Logs/app.log"), 10);
    b.fx.file(
        b.fx.env
            .home
            .join("Library/Logs/DiagnosticReports/crash.ips"),
        10,
    );
    assert_eq!(b.item("macos.trash").bytes, 1000);
    assert_eq!(b.item("macos.logs").files, 1);
    assert_eq!(b.item("macos.crash").files, 1);
    b.clean(&["macos.trash", "macos.logs"]);
    assert!(!b.fx.env.home.join(".Trash/old.dmg").exists());
    assert!(b.fx.env.home.join(".Trash").exists());
    assert!(b
        .fx
        .env
        .home
        .join("Library/Logs/DiagnosticReports/crash.ips")
        .exists());
}

// ------------------------------------------------------------------ secure deletion

#[cfg(unix)]
#[test]
fn secure_mode_overwrites_file_contents_before_unlinking() {
    for secure in [false, true] {
        let b = bed();
        let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
        let victim = p.cache.join("Cache/Cache_Data/data_0");
        // A second hard link keeps the inode alive so we can see what happened to its data.
        let alias = b.fx.env.home.join("alias-of-data_0");
        fs::hard_link(&victim, &alias).unwrap();
        let original = fs::read(&alias).unwrap();
        b.set(|s| {
            s.secure_delete.enabled = secure;
            s.secure_delete.passes = 3;
        });
        b.clean(&["chrome.cache"]);
        assert!(!victim.exists());
        let now = fs::read(&alias).unwrap();
        if secure {
            // the final DoD pass is random, and the file was truncated before unlinking
            assert!(now != original, "content should have been overwritten");
        } else {
            assert_eq!(now, original, "plain delete leaves the inode's data alone");
        }
    }
}

// ------------------------------------------------------------------ history, listing, options

#[test]
fn history_is_appended_per_run_and_capped() {
    let b = bed();
    b.fx.populate_chromium(Chromium::Chrome, "Default");
    let r = b.clean(&["chrome.cache", "chrome.history"]);
    let h = history::list(&b.ctx, None);
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].source, Source::Manual);
    assert_eq!(h[0].rule_ids, vec!["chrome.cache", "chrome.history"]);
    assert_eq!(h[0].total_files, r.total_files);
    assert_eq!(h[0].total_bytes, r.total_bytes);
    assert_eq!(r.history_id.as_deref(), Some(h[0].id.as_str()));
    assert!(h[0].at.ends_with('Z'));

    run_clean(
        &b.ctx,
        Some(vec!["chrome.cache".into()]),
        &CleanOptions::new(CloseBrowsers::Skip),
        Source::Scheduled,
        &Job::detached(),
    )
    .unwrap();
    let h = history::list(&b.ctx, Some(1));
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].source, Source::Scheduled, "newest first");

    for i in 0..110 {
        history::append(
            &b.ctx,
            HistoryEntry {
                id: format!("h{i}"),
                at: "2026-01-01T00:00:00Z".into(),
                total_bytes: i,
                total_files: 0,
                total_rows: 0,
                rule_ids: vec![],
                source: Source::Auto,
            },
        )
        .unwrap();
    }
    let h = history::list(&b.ctx, None);
    assert_eq!(h.len(), 100);
    assert_eq!(h[0].id, "h109");
    assert_eq!(h[99].id, "h10");
    let v = dispatch(
        &b.ctx,
        "cleaner.history",
        json!({"limit": 3}),
        &Job::detached(),
    )
    .unwrap();
    assert_eq!(v.as_array().unwrap().len(), 3);
    assert_eq!(v[0]["totalBytes"], 109);
    assert_eq!(v[0]["source"], "auto");
}

#[test]
fn list_rules_groups_by_category_and_reflects_selection() {
    let b = bed();
    let listing = list_rules(&b.ctx);
    let cats: Vec<_> = listing.categories.iter().map(|c| c.category).collect();
    assert_eq!(
        cats,
        vec![Category::Browser, Category::System, Category::Application]
    );
    let chrome = listing.categories[0]
        .groups
        .iter()
        .find(|g| g.group == "Google Chrome")
        .unwrap();
    assert_eq!(chrome.rules.len(), 7);
    let cache = chrome
        .rules
        .iter()
        .find(|r| r.id == "chrome.cache")
        .unwrap();
    assert!(cache.enabled && cache.default_enabled);
    let pw = chrome
        .rules
        .iter()
        .find(|r| r.id == "chrome.passwords")
        .unwrap();
    assert!(!pw.enabled);
    assert!(pw.warning.is_some());
    // linux only lists linux rules
    let all: Vec<&str> = listing
        .categories
        .iter()
        .flat_map(|c| {
            c.groups
                .iter()
                .flat_map(|g| g.rules.iter().map(|r| r.id.as_str()))
        })
        .collect();
    assert!(
        all.contains(&"linux.temp")
            && !all.contains(&"windows.temp")
            && !all.contains(&"macos.caches")
    );

    b.set(|s| s.selected_rules = Some(vec!["chrome.passwords".into(), "linux.temp".into()]));
    let v = dispatch(&b.ctx, "cleaner.list_rules", Value::Null, &Job::detached()).unwrap();
    let enabled: Vec<String> = v["categories"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["groups"].as_array().unwrap())
        .flat_map(|g| g["rules"].as_array().unwrap())
        .filter(|r| r["enabled"] == true)
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(enabled, vec!["chrome.passwords", "linux.temp"]);
    assert_eq!(v["categories"][0]["label"], "Browsers");
}

#[test]
fn default_selection_is_used_when_no_rule_ids_are_given() {
    let b = bed();
    b.fx.populate_chromium(Chromium::Chrome, "Default");
    let p = b.fx.chromium_profile(Chromium::Chrome, "Default");
    // defaults: cache/history/downloads/cookies on, passwords/autofill/session off
    let rep = analyze(&b.ctx, None, &Job::detached()).unwrap();
    let ids: Vec<&str> = rep.items.iter().map(|i| i.rule_id.as_str()).collect();
    assert!(ids.contains(&"chrome.cache") && ids.contains(&"chrome.cookies"));
    assert!(!ids.contains(&"chrome.passwords") && !ids.contains(&"chrome.session"));
    let v = dispatch(&b.ctx, "cleaner.clean", json!({}), &Job::detached()).unwrap();
    assert!(v["totalFiles"].as_u64().unwrap() >= 5);
    assert!(p.data.join("Sessions/Session_13300000000").exists());
    assert_eq!(
        q1(&p.data.join("Login Data"), "SELECT COUNT(*) FROM logins"),
        2
    );
    // Explicit selection in settings replaces the defaults.
    b.set(|s| s.selected_rules = Some(vec!["chrome.session".into()]));
    let rep = analyze(&b.ctx, None, &Job::detached()).unwrap();
    assert_eq!(rep.items.len(), 1);
    assert_eq!(rep.items[0].rule_id, "chrome.session");
}

#[test]
fn cancelled_clean_stops_before_touching_anything() {
    let b = bed();
    let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let job = Job::detached();
    job.token().cancel();
    let r = run_clean(
        &b.ctx,
        Some(vec!["chrome.cache".into()]),
        &CleanOptions::new(CloseBrowsers::Skip),
        Source::Manual,
        &job,
    )
    .unwrap();
    assert!(r.cancelled);
    assert!(r.results.is_empty());
    assert!(p.cache.join("Cache/Cache_Data/data_0").exists());
    assert!(r.history_id.is_none());
}

#[test]
fn progress_events_are_emitted_per_rule() {
    let b = bed();
    b.fx.populate_typical();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let job = Job::new(crate::job::CancelToken::new(), move |e| {
        s2.lock().unwrap().push(e)
    });
    analyze(
        &b.ctx,
        Some(vec!["chrome.cache".into(), "edge.cache".into()]),
        &job,
    )
    .unwrap();
    let ev = seen.lock().unwrap();
    assert!(ev.len() >= 3);
    assert!(ev.iter().all(|e| e.stage == "analyze"));
    assert_eq!(ev.last().unwrap().fraction, Some(1.0));
}

#[test]
fn clean_options_from_settings_maps_ask_to_skip_when_unattended() {
    let s = Settings::default();
    assert_eq!(
        CleanOptions::from_settings(&s, false).close_apps,
        CloseBrowsers::Ask
    );
    assert_eq!(
        CleanOptions::from_settings(&s, true).close_apps,
        CloseBrowsers::Skip
    );
    let s = Settings {
        close_browsers: CloseBrowsers::Always,
        ..Settings::default()
    };
    assert_eq!(
        CleanOptions::from_settings(&s, true).close_apps,
        CloseBrowsers::Always
    );
}

#[test]
fn env_paths_are_not_shared_between_beds() {
    // Safety net for the whole suite: fixtures live only below their own temp dir.
    let b = bed();
    let e: &Env = &b.fx.env;
    for p in [
        &e.home,
        &e.root,
        &e.data_dir,
        &e.temp_dir,
        &e.config_dir,
        &e.cache_dir,
    ] {
        assert!(p.starts_with(b.base()));
    }
}
