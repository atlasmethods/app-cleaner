use super::*;
use crate::api::dispatch;
use crate::error::ErrorCode;
use crate::job::CancelToken;
use crate::runner::{CmdOutput, MockRunner};
use filetime::FileTime;
use serde_json::json;
use std::fs;

struct Bed {
    _d: tempfile::TempDir,
    ctx: Ctx,
    /// Scanned tree (never under the protected home).
    tree: PathBuf,
}

fn bed() -> Bed {
    let d = tempfile::tempdir().unwrap();
    let ctx = Ctx::test(d.path(), MockRunner::new());
    let tree = d.path().join("tree");
    fs::create_dir_all(&tree).unwrap();
    fs::create_dir_all(&ctx.env.home).unwrap();
    Bed { _d: d, ctx, tree }
}

fn write(p: &Path, len: usize) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, vec![b'x'; len]).unwrap();
}

fn call(b: &Bed, method: &str, params: Value) -> Result<Value> {
    dispatch(&b.ctx, method, params, &Job::detached())
}

fn ok(b: &Bed, method: &str, params: Value) -> Value {
    call(b, method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn scan(b: &Bed, paths: &[&Path]) -> Value {
    ok(
        b,
        "disk_analyzer.scan",
        json!({"paths": paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>()}),
    )
}

/// A tree with known sizes:
/// pictures: a.jpg 100, sub/b.PNG 200 (300 in 2 files)
/// music: m.mp3 1000
/// documents: d.pdf 10, sub/deep/e.txt 20
/// video: v.mkv 5000
/// compressed: z.zip 40
/// email: mail.eml 5
/// other: noext 7, sub/x.rs 3
fn populate(root: &Path) {
    write(&root.join("a.jpg"), 100);
    write(&root.join("sub/b.PNG"), 200);
    write(&root.join("m.mp3"), 1000);
    write(&root.join("d.pdf"), 10);
    write(&root.join("sub/deep/e.txt"), 20);
    write(&root.join("v.mkv"), 5000);
    write(&root.join("z.zip"), 40);
    write(&root.join("mail.eml"), 5);
    write(&root.join("noext"), 7);
    write(&root.join("sub/x.rs"), 3);
}

fn totals(v: &Value, cat: &str) -> (u64, u64) {
    (
        v["totals"][cat]["files"].as_u64().unwrap(),
        v["totals"][cat]["bytes"].as_u64().unwrap(),
    )
}

#[test]
fn scan_totals_are_exact() {
    let b = bed();
    populate(&b.tree);
    let v = scan(&b, &[&b.tree]);
    assert_eq!(totals(&v, "pictures"), (2, 300));
    assert_eq!(totals(&v, "music"), (1, 1000));
    assert_eq!(totals(&v, "documents"), (2, 30));
    assert_eq!(totals(&v, "video"), (1, 5000));
    assert_eq!(totals(&v, "compressed"), (1, 40));
    assert_eq!(totals(&v, "email"), (1, 5));
    assert_eq!(totals(&v, "other"), (2, 10));
    assert_eq!(v["totalFiles"], 10);
    assert_eq!(v["totalBytes"], 300 + 1000 + 30 + 5000 + 40 + 5 + 10);
    assert_eq!(v["stored"], 10);
    assert_eq!(v["truncated"], false);
    assert_eq!(v["errors"]["count"], 0);
    assert!(v["scanId"].as_str().unwrap().starts_with("da-"));
    // every category key is present even when empty
    assert_eq!(v["totals"].as_object().unwrap().len(), 7);
}

#[test]
fn scan_can_be_limited_to_categories() {
    let b = bed();
    populate(&b.tree);
    let v = ok(
        &b,
        "disk_analyzer.scan",
        json!({"paths": [b.tree], "categories": ["music", "video"]}),
    );
    assert_eq!(v["totalFiles"], 2);
    assert_eq!(totals(&v, "music"), (1, 1000));
    assert_eq!(totals(&v, "pictures"), (0, 0));
    let bad = call(
        &b,
        "disk_analyzer.scan",
        json!({"paths": [b.tree], "categories": ["nonsense"]}),
    )
    .unwrap_err();
    assert_eq!(bad.code, ErrorCode::InvalidParams);
}

#[test]
fn scan_rejects_bad_paths() {
    let b = bed();
    for params in [
        json!({"paths": []}),
        json!({"paths": ["relative/dir"]}),
        json!({"paths": [b.tree.join("missing")]}),
    ] {
        assert!(call(&b, "disk_analyzer.scan", params).is_err());
    }
    write(&b.tree.join("f.txt"), 1);
    let e = call(
        &b,
        "disk_analyzer.scan",
        json!({"paths": [b.tree.join("f.txt")]}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams, "a file is not a folder");
    if b.ctx.env.os == crate::ctx::Os::Linux {
        let proc = b.ctx.env.sys_path("/proc");
        fs::create_dir_all(&proc).unwrap();
        let e = call(&b, "disk_analyzer.scan", json!({"paths": [proc]})).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams);
    }
}

#[cfg(unix)]
#[test]
fn symlinks_and_loops_are_never_followed_or_counted() {
    use std::os::unix::fs::symlink;
    let b = bed();
    write(&b.tree.join("real/one.txt"), 11);
    // loop: real/loop -> tree, and a link to a file outside the tree
    symlink(&b.tree, b.tree.join("real/loop")).unwrap();
    let outside = b.tree.parent().unwrap().join("outside");
    write(&outside.join("big.bin"), 9999);
    symlink(&outside, b.tree.join("out_link")).unwrap();
    symlink(outside.join("big.bin"), b.tree.join("file_link")).unwrap();
    symlink(b.tree.join("missing"), b.tree.join("dangling")).unwrap();
    let v = scan(&b, &[&b.tree]);
    assert_eq!(v["totalFiles"], 1);
    assert_eq!(v["totalBytes"], 11);
    assert_eq!(v["errors"]["count"], 0);
}

#[cfg(unix)]
#[test]
fn hard_links_count_once() {
    let b = bed();
    write(&b.tree.join("orig.bin"), 500);
    fs::hard_link(b.tree.join("orig.bin"), b.tree.join("sub_link.bin")).unwrap();
    write(&b.tree.join("other.bin"), 25);
    let v = scan(&b, &[&b.tree]);
    assert_eq!(v["totalFiles"], 2);
    assert_eq!(v["totalBytes"], 525);
}

#[test]
fn nested_and_repeated_roots_are_not_double_counted() {
    let b = bed();
    populate(&b.tree);
    let v = ok(
        &b,
        "disk_analyzer.scan",
        json!({"paths": [b.tree.join("sub"), b.tree, b.tree.join("sub/deep"), b.tree]}),
    );
    assert_eq!(v["roots"].as_array().unwrap().len(), 1);
    assert_eq!(v["totalFiles"], 10);
}

#[test]
fn many_files_are_bounded_but_totals_stay_exact() {
    let b = bed();
    for i in 0..30usize {
        write(&b.tree.join(format!("d{}/f{i}.bin", i % 3)), 10 + i);
    }
    let roots = vec![b.tree.canonicalize().unwrap()];
    let opts = ScanOptions {
        cats: [true; 7],
        max_files: 5,
    };
    let data = scan::run_scan(&b.ctx, "t".into(), roots, &opts, &Job::detached()).unwrap();
    assert_eq!(data.total_files(), 30);
    assert_eq!(data.total_bytes(), (0..30u64).map(|i| 10 + i).sum::<u64>());
    assert!(data.truncated);
    assert_eq!(data.files.len(), 5);
    let sizes: Vec<u64> = data.files.iter().map(|f| f.bytes).collect();
    assert_eq!(
        sizes,
        vec![39, 38, 37, 36, 35],
        "the biggest files are kept"
    );
    // folder sizes cover ALL files, not only the stored ones
    assert_eq!(data.nodes[0].sub_files, 30);
}

#[test]
fn cancelled_scan_returns_cancelled() {
    let b = bed();
    populate(&b.tree);
    let token = CancelToken::new();
    token.cancel();
    let job = Job::with_token(token);
    let e = scan::run_scan(
        &b.ctx,
        "t".into(),
        vec![b.tree.canonicalize().unwrap()],
        &ScanOptions::all(),
        &job,
    )
    .err()
    .unwrap();
    assert_eq!(e.code, ErrorCode::Cancelled);
}

#[cfg(unix)]
#[test]
fn unreadable_folders_are_counted_as_errors_not_failures() {
    use std::os::unix::fs::PermissionsExt;
    let b = bed();
    write(&b.tree.join("ok.txt"), 4);
    let locked = b.tree.join("locked");
    write(&locked.join("secret.txt"), 4);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = fs::read_dir(&locked).is_ok(); // running as root
    let v = scan(&b, &[&b.tree]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if readable_anyway {
        assert_eq!(v["totalFiles"], 2);
    } else {
        assert_eq!(v["totalFiles"], 1);
        assert_eq!(v["errors"]["count"], 1);
        assert!(v["errors"]["samples"][0]
            .as_str()
            .unwrap()
            .contains("locked"));
    }
}

// ---------------------------------------------------------------- files paging

fn list(b: &Bed, id: &str, extra: Value) -> Value {
    let mut p = json!({"scanId": id});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    ok(b, "disk_analyzer.files", p)
}

fn names(v: &Value) -> Vec<String> {
    v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn files_paging_and_sorting() {
    let b = bed();
    populate(&b.tree);
    // distinct mtimes: newest = z.zip, then v.mkv, ...
    let order = [
        "z.zip", "v.mkv", "m.mp3", "a.jpg", "d.pdf", "mail.eml", "noext", "b.PNG", "e.txt", "x.rs",
    ];
    for (i, n) in order.iter().enumerate() {
        let p = walkdir::WalkDir::new(&b.tree)
            .into_iter()
            .flatten()
            .find(|e| e.file_name() == *n)
            .unwrap();
        filetime::set_file_mtime(
            p.path(),
            FileTime::from_unix_time(2_000_000_000 - (i as i64) * 1000, 0),
        )
        .unwrap();
    }
    let id = scan(&b, &[&b.tree])["scanId"].as_str().unwrap().to_string();

    let by_size = list(&b, &id, json!({}));
    assert_eq!(by_size["total"], 10);
    assert_eq!(
        names(&by_size),
        vec![
            "v.mkv", "m.mp3", "b.PNG", "a.jpg", "z.zip", "e.txt", "d.pdf", "noext", "mail.eml",
            "x.rs"
        ]
    );
    let by_name = list(&b, &id, json!({"sort": "name"}));
    assert_eq!(
        names(&by_name),
        vec![
            "a.jpg", "b.PNG", "d.pdf", "e.txt", "m.mp3", "mail.eml", "noext", "v.mkv", "x.rs",
            "z.zip"
        ]
    );
    let by_mod = list(&b, &id, json!({"sort": "modified"}));
    assert_eq!(names(&by_mod), order.to_vec());

    let page2 = list(&b, &id, json!({"sort": "name", "offset": 3, "limit": 4}));
    assert_eq!(names(&page2), vec!["e.txt", "m.mp3", "mail.eml", "noext"]);
    assert_eq!(page2["total"], 10, "total is independent of the page");
    let past_end = list(&b, &id, json!({"offset": 50}));
    assert!(names(&past_end).is_empty());

    let pics = list(&b, &id, json!({"category": "pictures"}));
    assert_eq!(names(&pics), vec!["b.PNG", "a.jpg"]);
    assert_eq!(pics["total"], 2);
    assert_eq!(pics["totalBytes"], 300);
    assert_eq!(pics["files"][0]["category"], "pictures");
    assert_eq!(pics["files"][0]["bytes"], 200);

    let in_sub = list(&b, &id, json!({"folder": b.tree.join("sub")}));
    assert_eq!(names(&in_sub), vec!["b.PNG", "e.txt", "x.rs"]);
    let in_deep = list(
        &b,
        &id,
        json!({"folder": b.tree.join("sub/deep"), "category": "documents"}),
    );
    assert_eq!(names(&in_deep), vec!["e.txt"]);
    // "sub" must not match a sibling folder that merely starts with the same letters
    write(&b.tree.join("subway/s.txt"), 1);
    let id2 = scan(&b, &[&b.tree])["scanId"].as_str().unwrap().to_string();
    let only_sub = list(&b, &id2, json!({"folder": b.tree.join("sub")}));
    assert_eq!(only_sub["total"], 3);
}

#[test]
fn unknown_scan_id_is_not_found_everywhere() {
    let b = bed();
    for (m, p) in [
        ("disk_analyzer.files", json!({"scanId": "nope"})),
        ("disk_analyzer.tree", json!({"scanId": "nope"})),
        (
            "disk_analyzer.delete",
            json!({"scanId": "nope", "paths": ["/x"]}),
        ),
    ] {
        assert_eq!(call(&b, m, p).unwrap_err().code, ErrorCode::NotFound, "{m}");
    }
}

#[test]
fn only_the_last_three_scans_are_kept() {
    let b = bed();
    write(&b.tree.join("a.txt"), 1);
    let ids: Vec<String> = (0..4)
        .map(|_| scan(&b, &[&b.tree])["scanId"].as_str().unwrap().to_string())
        .collect();
    let gone = call(&b, "disk_analyzer.files", json!({"scanId": ids[0]})).unwrap_err();
    assert_eq!(gone.code, ErrorCode::NotFound);
    for id in &ids[1..] {
        list(&b, id, json!({}));
    }
}

// ---------------------------------------------------------------- tree

#[test]
fn tree_aggregates_folders() {
    let b = bed();
    populate(&b.tree);
    let id = scan(&b, &[&b.tree])["scanId"].as_str().unwrap().to_string();
    let root = ok(&b, "disk_analyzer.tree", json!({"scanId": id}));
    let total = 300 + 1000 + 30 + 5000 + 40 + 5 + 10;
    assert_eq!(root["bytes"], total);
    assert_eq!(root["files"], 10);
    // files directly in the root: m.mp3 d.pdf v.mkv z.zip mail.eml noext a.jpg
    assert_eq!(root["ownFiles"], 7);
    assert_eq!(root["ownBytes"], 1000 + 10 + 5000 + 40 + 5 + 7 + 100);
    assert_eq!(root["canGoUp"], false);
    let kids = root["children"].as_array().unwrap();
    assert_eq!(kids.len(), 1);
    assert_eq!(kids[0]["name"], "sub");
    assert_eq!(kids[0]["bytes"], 200 + 20 + 3);
    assert_eq!(kids[0]["files"], 3);

    let sub = ok(
        &b,
        "disk_analyzer.tree",
        json!({"scanId": id, "path": kids[0]["path"]}),
    );
    assert_eq!(sub["ownFiles"], 2);
    assert_eq!(sub["canGoUp"], true);
    assert_eq!(sub["parent"], root["path"]);
    assert_eq!(
        root["parent"],
        Value::Null,
        "the parent of a root is the top level"
    );
    assert_eq!(sub["children"][0]["name"], "deep");
    let deep = ok(
        &b,
        "disk_analyzer.tree",
        json!({"scanId": id, "path": sub["children"][0]["path"]}),
    );
    assert_eq!(deep["bytes"], 20);
    assert_eq!(deep["parent"], sub["path"]);
    assert_eq!(deep["children"].as_array().unwrap().len(), 0);

    let missing = call(
        &b,
        "disk_analyzer.tree",
        json!({"scanId": id, "path": b.tree.join("nope")}),
    )
    .unwrap_err();
    assert_eq!(missing.code, ErrorCode::NotFound);
    let elsewhere = call(
        &b,
        "disk_analyzer.tree",
        json!({"scanId": id, "path": "/definitely/not/scanned"}),
    )
    .unwrap_err();
    assert_eq!(elsewhere.code, ErrorCode::NotFound);
}

#[test]
fn tree_children_are_sorted_by_size() {
    let b = bed();
    write(&b.tree.join("small/a.bin"), 1);
    write(&b.tree.join("big/a.bin"), 1000);
    write(&b.tree.join("mid/a.bin"), 100);
    let id = scan(&b, &[&b.tree])["scanId"].as_str().unwrap().to_string();
    let root = ok(&b, "disk_analyzer.tree", json!({"scanId": id}));
    let order: Vec<&str> = root["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(order, vec!["big", "mid", "small"]);
}

#[test]
fn multi_root_scans_have_a_virtual_top_level() {
    let b = bed();
    let a = b.tree.join("A");
    let c = b.tree.join("C");
    write(&a.join("f1.bin"), 10);
    write(&c.join("f2.bin"), 90);
    let id = scan(&b, &[&a, &c])["scanId"].as_str().unwrap().to_string();
    let top = ok(&b, "disk_analyzer.tree", json!({"scanId": id}));
    assert_eq!(top["path"], Value::Null);
    assert_eq!(top["bytes"], 100);
    let kids = top["children"].as_array().unwrap();
    assert_eq!(kids.len(), 2);
    assert_eq!(kids[0]["bytes"], 90, "biggest root first");
    let one = ok(
        &b,
        "disk_analyzer.tree",
        json!({"scanId": id, "path": kids[0]["path"]}),
    );
    assert_eq!(one["canGoUp"], true);
    assert_eq!(one["bytes"], 90);
}

// ---------------------------------------------------------------- delete

fn set_secure(b: &Bed, enabled: bool) {
    settings::update(&b.ctx, |s| {
        s.secure_delete.enabled = enabled;
        s.secure_delete.passes = 1;
    })
    .unwrap();
}

#[test]
fn delete_removes_files_and_updates_the_scan() {
    let b = bed();
    populate(&b.tree);
    let id = scan(&b, &[&b.tree])["scanId"].as_str().unwrap().to_string();
    let victim = b.tree.join("sub/b.PNG");
    let other = b.tree.join("v.mkv");
    let r = ok(
        &b,
        "disk_analyzer.delete",
        json!({"scanId": id, "paths": [victim, other]}),
    );
    assert_eq!(r["deleted"], 2);
    assert_eq!(r["freedBytes"], 5200);
    assert!(r["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|x| x["ok"] == true));
    assert!(!victim.exists() && !other.exists());
    assert!(b.tree.join("a.jpg").exists());
    assert_eq!(totals(&r, "pictures"), (1, 100));
    assert_eq!(totals(&r, "video"), (0, 0));
    assert_eq!(r["totalFiles"], 8);
    // listing and tree reflect it without a rescan
    let l = list(&b, &id, json!({}));
    assert_eq!(l["total"], 8);
    assert!(!names(&l).contains(&"v.mkv".to_string()));
    let root = ok(&b, "disk_analyzer.tree", json!({"scanId": id}));
    assert_eq!(root["files"], 8);
    assert_eq!(root["bytes"], 6385 - 5200);
}

#[test]
fn delete_refuses_paths_that_are_not_in_the_scan() {
    let b = bed();
    populate(&b.tree);
    let bystander = b.tree.parent().unwrap().join("bystander.txt");
    write(&bystander, 5);
    let listed_but_other_scan = b.tree.join("a.jpg");
    let id = scan(&b, &[&b.tree.join("sub")])["scanId"]
        .as_str()
        .unwrap()
        .to_string();
    let traversal = format!("{}/sub/../a.jpg", b.tree.display());
    let r = ok(
        &b,
        "disk_analyzer.delete",
        json!({"scanId": id, "paths": [
            bystander,
            listed_but_other_scan,
            traversal,
            "relative.txt",
            b.tree.join("sub"),
        ]}),
    );
    assert_eq!(r["deleted"], 0);
    for x in r["results"].as_array().unwrap() {
        assert_eq!(x["ok"], false, "{x}");
        assert_eq!(x["error"], "not part of this analysis");
    }
    assert!(bystander.exists());
    assert!(b.tree.join("a.jpg").exists());
    assert!(b.tree.join("sub").exists());
}

#[test]
fn delete_refuses_files_changed_since_the_scan() {
    let b = bed();
    write(&b.tree.join("doc.txt"), 10);
    write(&b.tree.join("grown.txt"), 10);
    write(&b.tree.join("touched.txt"), 10);
    write(&b.tree.join("swapped.txt"), 10);
    let id = scan(&b, &[&b.tree])["scanId"].as_str().unwrap().to_string();
    fs::write(b.tree.join("grown.txt"), vec![b'y'; 99]).unwrap();
    filetime::set_file_mtime(
        b.tree.join("touched.txt"),
        FileTime::from_unix_time(1_000_000_000, 0),
    )
    .unwrap();
    fs::remove_file(b.tree.join("swapped.txt")).unwrap();
    fs::create_dir(b.tree.join("swapped.txt")).unwrap();
    let paths: Vec<PathBuf> = ["doc.txt", "grown.txt", "touched.txt", "swapped.txt"]
        .iter()
        .map(|n| b.tree.join(n))
        .collect();
    let r = ok(
        &b,
        "disk_analyzer.delete",
        json!({"scanId": id, "paths": paths}),
    );
    let res = r["results"].as_array().unwrap();
    assert_eq!(res[0]["ok"], true);
    for x in &res[1..] {
        assert_eq!(x["ok"], false, "{x}");
    }
    assert!(res[1]["error"].as_str().unwrap().contains("changed"));
    assert!(res[3]["error"].as_str().unwrap().contains("no longer"));
    assert!(!b.tree.join("doc.txt").exists());
    assert!(b.tree.join("grown.txt").exists());
    assert!(b.tree.join("touched.txt").exists());
    assert!(b.tree.join("swapped.txt").is_dir());
}

#[cfg(unix)]
#[test]
fn a_file_replaced_by_a_symlink_is_not_followed() {
    use std::os::unix::fs::symlink;
    let b = bed();
    write(&b.tree.join("victim.txt"), 10);
    let precious = b.tree.parent().unwrap().join("precious.txt");
    write(&precious, 10);
    let id = scan(&b, &[&b.tree])["scanId"].as_str().unwrap().to_string();
    fs::remove_file(b.tree.join("victim.txt")).unwrap();
    symlink(&precious, b.tree.join("victim.txt")).unwrap();
    let r = ok(
        &b,
        "disk_analyzer.delete",
        json!({"scanId": id, "paths": [b.tree.join("victim.txt")]}),
    );
    assert_eq!(r["results"][0]["ok"], false);
    assert!(precious.exists());
}

#[cfg(unix)]
#[test]
fn a_parent_swapped_for_a_symlink_cannot_escape_the_root() {
    use std::os::unix::fs::symlink;
    let b = bed();
    write(&b.tree.join("dir/f.txt"), 10);
    let outside = b.tree.parent().unwrap().join("outside");
    write(&outside.join("f.txt"), 10);
    let id = scan(&b, &[&b.tree])["scanId"].as_str().unwrap().to_string();
    // after the scan, `dir` becomes a link to a folder outside the scanned root that
    // contains a file of identical size and mtime
    let mt = fs::metadata(b.tree.join("dir/f.txt"))
        .unwrap()
        .modified()
        .unwrap();
    filetime::set_file_mtime(outside.join("f.txt"), FileTime::from_system_time(mt)).unwrap();
    fs::remove_dir_all(b.tree.join("dir")).unwrap();
    symlink(&outside, b.tree.join("dir")).unwrap();
    let r = ok(
        &b,
        "disk_analyzer.delete",
        json!({"scanId": id, "paths": [b.tree.join("dir/f.txt")]}),
    );
    assert_eq!(r["results"][0]["ok"], false, "{r}");
    assert!(outside.join("f.txt").exists());
}

#[test]
fn delete_refuses_protected_locations() {
    let b = bed();
    // A scan of the whole home directory: the Documents folder itself and .ssh are
    // protected, ordinary files below are fine.
    let docs = b.ctx.env.home.join("Documents");
    write(&docs.join("keep.txt"), 3);
    write(&b.ctx.env.home.join(".ssh/id_ed25519"), 3);
    let id = scan(&b, &[&b.ctx.env.home])["scanId"]
        .as_str()
        .unwrap()
        .to_string();
    let r = ok(
        &b,
        "disk_analyzer.delete",
        json!({"scanId": id, "paths": [
            b.ctx.env.home.join(".ssh/id_ed25519"),
            docs.join("keep.txt"),
        ]}),
    );
    let res = r["results"].as_array().unwrap();
    assert_eq!(res[0]["ok"], false, "{r}");
    assert!(res[0]["error"].as_str().unwrap().contains("protected"));
    assert_eq!(res[1]["ok"], true, "files inside Documents may go: {r}");
    assert!(b.ctx.env.home.join(".ssh/id_ed25519").exists());
}

#[test]
fn delete_honours_the_exclusion_list() {
    let b = bed();
    write(&b.tree.join("keep/a.txt"), 3);
    write(&b.tree.join("go.txt"), 3);
    settings::update(&b.ctx, |s| {
        s.exclude.push(settings::ExcludeEntry {
            id: "e1".into(),
            pattern: b.tree.join("keep").to_string_lossy().into_owned(),
        });
    })
    .unwrap();
    let id = scan(&b, &[&b.tree])["scanId"].as_str().unwrap().to_string();
    let r = ok(
        &b,
        "disk_analyzer.delete",
        json!({"scanId": id, "paths": [b.tree.join("keep/a.txt"), b.tree.join("go.txt")]}),
    );
    assert_eq!(r["results"][0]["ok"], false);
    assert_eq!(r["results"][1]["ok"], true);
    assert!(b.tree.join("keep/a.txt").exists());
}

#[test]
fn delete_uses_secure_deletion_when_enabled() {
    let b = bed();
    write(&b.tree.join("a.txt"), 2048);
    set_secure(&b, true);
    let id = scan(&b, &[&b.tree])["scanId"].as_str().unwrap().to_string();
    let r = ok(
        &b,
        "disk_analyzer.delete",
        json!({"scanId": id, "paths": [b.tree.join("a.txt")]}),
    );
    assert_eq!(r["results"][0]["ok"], true);
    assert_eq!(r["freedBytes"], 2048);
    // Nothing (not even a renamed remnant) is left in the folder.
    assert_eq!(fs::read_dir(&b.tree).unwrap().count(), 0);
}

#[test]
fn delete_reports_files_already_gone_without_claiming_space() {
    let b = bed();
    write(&b.tree.join("a.txt"), 10);
    let id = scan(&b, &[&b.tree])["scanId"].as_str().unwrap().to_string();
    fs::remove_file(b.tree.join("a.txt")).unwrap();
    let r = ok(
        &b,
        "disk_analyzer.delete",
        json!({"scanId": id, "paths": [b.tree.join("a.txt")]}),
    );
    // the precheck sees it missing: reported as not deletable, and stays in the list
    assert_eq!(r["results"][0]["ok"], false);
    assert_eq!(r["freedBytes"], 0);
}

#[test]
fn delete_with_no_paths_is_invalid() {
    let b = bed();
    write(&b.tree.join("a.txt"), 1);
    let id = scan(&b, &[&b.tree])["scanId"].as_str().unwrap().to_string();
    let e = call(
        &b,
        "disk_analyzer.delete",
        json!({"scanId": id, "paths": []}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
}

// ---------------------------------------------------------------- open_folder

fn open_bed(os: crate::ctx::Os, program: &str) -> (Bed, MockRunner) {
    let mut b = bed();
    let m = MockRunner::new();
    b.ctx = Ctx::test(b._d.path(), m.clone());
    b.ctx.env.os = os;
    if !program.is_empty() {
        m.with_program(program);
    }
    (b, m)
}

#[test]
fn open_folder_linux_opens_the_containing_folder_never_the_file() {
    let (b, m) = open_bed(Os::Linux, "xdg-open");
    write(&b.tree.join("movie.mkv"), 1);
    let tree = b.tree.to_string_lossy().into_owned();
    m.on("xdg-open", &[&tree], CmdOutput::ok(""));
    let file = b.tree.join("movie.mkv");
    let r = ok(&b, "disk_analyzer.open_folder", json!({"path": file}));
    assert_eq!(r["opened"], tree);
    ok(&b, "disk_analyzer.open_folder", json!({"path": b.tree}));
    assert_eq!(m.calls().len(), 2);
    assert!(m.calls().iter().all(|c| c.1 == vec![tree.clone()]));
}

#[test]
fn open_folder_macos_and_windows_commands() {
    let (b, m) = open_bed(Os::MacOs, "open");
    write(&b.tree.join("f.txt"), 1);
    let f = b.tree.join("f.txt").to_string_lossy().into_owned();
    let t = b.tree.to_string_lossy().into_owned();
    m.on("open", &["-R", &f], CmdOutput::ok(""));
    m.on("open", &[&t], CmdOutput::ok(""));
    ok(&b, "disk_analyzer.open_folder", json!({"path": f}));
    ok(&b, "disk_analyzer.open_folder", json!({"path": t}));

    let (b, m) = open_bed(Os::Windows, "explorer");
    write(&b.tree.join("f.txt"), 1);
    let f = b.tree.join("f.txt").to_string_lossy().into_owned();
    // explorer.exe exits with 1 even on success
    m.on(
        "explorer",
        &[&format!("/select,{f}")],
        CmdOutput::failed(1, ""),
    );
    ok(&b, "disk_analyzer.open_folder", json!({"path": f}));
}

#[test]
fn open_folder_errors() {
    let (b, _m) = open_bed(Os::Linux, "");
    assert_eq!(
        call(
            &b,
            "disk_analyzer.open_folder",
            json!({"path": b.tree.join("nope")})
        )
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        call(&b, "disk_analyzer.open_folder", json!({"path": "rel"}))
            .unwrap_err()
            .code,
        ErrorCode::InvalidParams
    );
    assert_eq!(
        call(&b, "disk_analyzer.open_folder", json!({"path": b.tree}))
            .unwrap_err()
            .code,
        ErrorCode::Unsupported,
        "no xdg-open on PATH"
    );
    let (b, m) = open_bed(Os::Linux, "xdg-open");
    m.on(
        "xdg-open",
        &[&b.tree.to_string_lossy()],
        CmdOutput::failed(3, "no handler"),
    );
    let e = call(&b, "disk_analyzer.open_folder", json!({"path": b.tree})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Io);
}

#[test]
fn list_drives_returns_real_volumes() {
    let b = bed();
    let v = ok(&b, "disk_analyzer.list_drives", json!({}));
    for d in v.as_array().unwrap() {
        assert!(d["total"].as_u64().unwrap() > 0);
        assert!(d["mount"].as_str().is_some());
        assert!(d["fs"].as_str().is_some());
        assert!(d.get("removable").is_some());
    }
}

// ---------------------------------------------------------------- real filesystem boundary

/// A tmpfs mounted inside the scanned tree is a different filesystem: neither the
/// analyzer nor the duplicate finder may descend into it. Run with
/// `cargo test -p sweep-core -- --ignored tmpfs` as root (scripts/check.sh does).
#[test]
#[ignore = "needs root and the ability to mount a tmpfs"]
fn tmpfs_mount_inside_the_tree_is_not_entered() {
    let b = bed();
    write(&b.tree.join("outside.txt"), 10);
    let mnt = b.tree.join("mnt");
    fs::create_dir_all(&mnt).unwrap();
    let Some(_t) = crate::features::wiper::tests::Tmpfs::mount_at(&mnt, 8) else {
        eprintln!("SKIPPED: cannot mount a tmpfs here");
        return;
    };
    write(&mnt.join("inside.txt"), 1000);
    write(&mnt.join("inside_copy.txt"), 1000);
    let v = scan(&b, &[&b.tree]);
    assert_eq!(v["totalFiles"], 1, "{v}");
    assert_eq!(v["totalBytes"], 10);
    // scanning the mount itself works: it is the root of that walk
    let v = scan(&b, &[&mnt]);
    assert_eq!(v["totalFiles"], 2);
    // duplicates
    let r = ok(
        &b,
        "duplicates.scan",
        json!({"paths": [b.tree], "matchBy": {"size": true}}),
    );
    assert_eq!(r["scannedFiles"], 1);
}
