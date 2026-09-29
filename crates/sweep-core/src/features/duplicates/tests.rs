use super::*;
use crate::api::dispatch;
use crate::error::ErrorCode;
use crate::job::CancelToken;
use crate::runner::MockRunner;
use filetime::FileTime;
use serde_json::json;

struct Bed {
    _d: tempfile::TempDir,
    ctx: Ctx,
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

fn put(p: &Path, content: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn set_mtime(p: &Path, secs: i64) {
    filetime::set_file_mtime(p, FileTime::from_unix_time(secs, 0)).unwrap();
}

fn call(b: &Bed, method: &str, params: Value) -> Result<Value> {
    dispatch(&b.ctx, method, params, &Job::detached())
}

fn ok(b: &Bed, method: &str, params: Value) -> Value {
    call(b, method, params).unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn content_scan(b: &Bed) -> Value {
    ok(
        b,
        "duplicates.scan",
        json!({"paths": [b.tree], "matchBy": {"content": true}}),
    )
}

/// Group member file names, each group sorted, groups sorted.
fn shape(v: &Value, base: &Path) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| {
            let mut names: Vec<String> = g["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| {
                    Path::new(f["path"].as_str().unwrap())
                        .strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            names.sort();
            names
        })
        .collect();
    out.sort();
    out
}

fn v(items: &[&[&str]]) -> Vec<Vec<String>> {
    items
        .iter()
        .map(|g| g.iter().map(|s| s.to_string()).collect())
        .collect()
}

fn big(head: u8, mid: u8, tail: u8) -> Vec<u8> {
    // > 2 * 4096 so the partial hash does not cover the whole file
    let mut d = vec![head; 4096];
    d.extend(vec![mid; 20_000]);
    d.extend(vec![tail; 4096]);
    d
}

#[test]
fn content_groups_are_exact() {
    let b = bed();
    let t = &b.tree;
    // identical pair + a third copy elsewhere
    put(&t.join("a/report.txt"), b"the same content");
    put(&t.join("b/copy of report.txt"), b"the same content");
    put(&t.join("c/x.bin"), b"the same content");
    // same size, different content (small)
    put(&t.join("a/s1.txt"), b"AAAAAAAAAA");
    put(&t.join("a/s2.txt"), b"AAAAAAAAAB");
    // same head and tail and size, different middle (large)
    put(&t.join("big/h1.bin"), &big(1, 2, 3));
    put(&t.join("big/h2.bin"), &big(1, 9, 3));
    // large identical pair
    put(&t.join("big/same1.bin"), &big(4, 5, 6));
    put(&t.join("big/same2.bin"), &big(4, 5, 6));
    // unique
    put(&t.join("u.txt"), b"lonely");
    let r = content_scan(&b);
    assert_eq!(
        shape(&r, t),
        v(&[
            &["a/report.txt", "b/copy of report.txt", "c/x.bin"],
            &["big/same1.bin", "big/same2.bin"],
        ])
    );
    assert_eq!(r["totalGroups"], 2);
    assert_eq!(r["totalFiles"], 5);
    let g = &r["groups"];
    // sorted by wasted bytes: the big pair wastes 28192, the small triple 32
    assert_eq!(g[0]["wastedBytes"], 28_192);
    assert_eq!(g[1]["wastedBytes"], 32);
    assert_eq!(g[1]["key"]["bytes"], 16);
    assert_eq!(g[1]["key"]["hash"].as_str().unwrap().len(), 64);
    assert_eq!(r["wastedBytes"], 28_192 + 32);
    assert_eq!(r["scannedFiles"], 10);
    assert_eq!(g[0]["groupId"], "g0");
    assert_eq!(g[1]["groupId"], "g1");
}

#[test]
fn identical_prefix_and_suffix_but_different_middle_is_not_a_duplicate() {
    let b = bed();
    let mut a = vec![7u8; 100_000];
    let mut c = a.clone();
    c[50_000] = 8;
    put(&b.tree.join("a.bin"), &a);
    put(&b.tree.join("c.bin"), &c);
    assert_eq!(content_scan(&b)["totalGroups"], 0);
    a[50_000] = 8;
    put(&b.tree.join("d.bin"), &a);
    assert_eq!(content_scan(&b)["totalGroups"], 1);
}

#[test]
fn zero_byte_files_are_skipped_unless_asked() {
    let b = bed();
    put(&b.tree.join("e1"), b"");
    put(&b.tree.join("e2"), b"");
    assert_eq!(content_scan(&b)["totalGroups"], 0);
    let r = ok(
        &b,
        "duplicates.scan",
        json!({"paths": [b.tree], "matchBy": {"content": true}, "skipZeroByte": false}),
    );
    assert_eq!(r["totalGroups"], 1);
    assert_eq!(r["groups"][0]["wastedBytes"], 0);
}

#[cfg(unix)]
#[test]
fn hard_links_are_one_file_and_never_a_duplicate_of_themselves() {
    let b = bed();
    let t = &b.tree;
    put(&t.join("orig.dat"), b"hard linked data");
    fs::hard_link(t.join("orig.dat"), t.join("link1.dat")).unwrap();
    fs::hard_link(t.join("orig.dat"), t.join("sub/link2.dat")).unwrap_or_else(|_| {
        fs::create_dir_all(t.join("sub")).unwrap();
        fs::hard_link(t.join("orig.dat"), t.join("sub/link2.dat")).unwrap();
    });
    // only hard links of one file: no duplicates at all, in every mode
    for m in [
        json!({"content": true}),
        json!({"size": true}),
        json!({"modified": true}),
        json!({"size": true, "modified": true}),
    ] {
        let r = ok(&b, "duplicates.scan", json!({"paths": [t], "matchBy": m}));
        assert_eq!(r["totalGroups"], 0, "{m}");
        assert_eq!(r["scannedFiles"], 3);
    }
    // a genuine second copy: the group has exactly TWO entries, one of the linked names
    put(&t.join("real_copy.dat"), b"hard linked data");
    let r = content_scan(&b);
    assert_eq!(r["totalGroups"], 1);
    let files = r["groups"][0]["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(r["groups"][0]["wastedBytes"], 16, "one copy's worth");
}

#[cfg(unix)]
#[test]
fn symlinks_are_skipped() {
    use std::os::unix::fs::symlink;
    let b = bed();
    put(&b.tree.join("file.txt"), b"content!");
    symlink(b.tree.join("file.txt"), b.tree.join("alias.txt")).unwrap();
    symlink(&b.tree, b.tree.join("loop")).unwrap();
    let r = content_scan(&b);
    assert_eq!(r["totalGroups"], 0);
    assert_eq!(r["scannedFiles"], 1);
    // following links still never reports a link or the file twice
    let r = ok(
        &b,
        "duplicates.scan",
        json!({"paths": [b.tree], "matchBy": {"content": true}, "followLinks": true}),
    );
    assert_eq!(r["totalGroups"], 0);
}

#[test]
fn name_size_and_modified_modes() {
    let b = bed();
    let t = &b.tree;
    put(&t.join("d1/photo.jpg"), b"12345");
    put(&t.join("d2/photo.jpg"), b"abcdefgh"); // same name, different size
    put(&t.join("d3/other.jpg"), b"12345"); // same size as d1
    put(&t.join("d4/PHOTO.JPG"), b"zzz");
    set_mtime(&t.join("d1/photo.jpg"), 1_600_000_000);
    set_mtime(&t.join("d3/other.jpg"), 1_600_000_000);
    set_mtime(&t.join("d2/photo.jpg"), 1_500_000_000);
    set_mtime(&t.join("d4/PHOTO.JPG"), 1_400_000_000);

    let by = |m: Value| ok(&b, "duplicates.scan", json!({"paths": [t], "matchBy": m}));

    let name = by(json!({"name": true}));
    if b.ctx.env.os == Os::Linux {
        assert_eq!(shape(&name, t), v(&[&["d1/photo.jpg", "d2/photo.jpg"]]));
    }
    assert_eq!(name["groups"][0]["key"]["name"], "photo.jpg");

    let size = by(json!({"size": true}));
    assert_eq!(shape(&size, t), v(&[&["d1/photo.jpg", "d3/other.jpg"]]));
    assert_eq!(size["groups"][0]["key"]["bytes"], 5);
    assert_eq!(size["groups"][0]["wastedBytes"], 5);

    let modified = by(json!({"modified": true}));
    assert_eq!(shape(&modified, t), v(&[&["d1/photo.jpg", "d3/other.jpg"]]));

    let both = by(json!({"size": true, "modified": true}));
    assert_eq!(shape(&both, t), v(&[&["d1/photo.jpg", "d3/other.jpg"]]));

    let name_size = by(json!({"name": true, "size": true}));
    assert_eq!(name_size["totalGroups"], 0);

    let name_content = by(json!({"name": true, "content": true}));
    assert_eq!(name_content["totalGroups"], 0);
    put(&t.join("d5/photo.jpg"), b"12345");
    let name_content = by(json!({"name": true, "content": true}));
    assert_eq!(
        shape(&name_content, t),
        v(&[&["d1/photo.jpg", "d5/photo.jpg"]])
    );
}

#[test]
fn names_compare_case_insensitively_on_windows_and_macos_only() {
    let mut b = bed();
    put(&b.tree.join("a/Photo.JPG"), b"1");
    put(&b.tree.join("b/photo.jpg"), b"22");
    let run = |b: &Bed| {
        ok(
            b,
            "duplicates.scan",
            json!({"paths": [b.tree], "matchBy": {"name": true}}),
        )["totalGroups"]
            .as_u64()
            .unwrap()
    };
    b.ctx.env.os = Os::Linux;
    assert_eq!(run(&b), 0);
    b.ctx.env.os = Os::Windows;
    assert_eq!(run(&b), 1);
    b.ctx.env.os = Os::MacOs;
    assert_eq!(run(&b), 1);
}

#[test]
fn a_criterion_is_required_and_sizes_are_checked() {
    let b = bed();
    for params in [
        json!({"paths": [b.tree], "matchBy": {}}),
        json!({"paths": [b.tree], "matchBy": {"content": true}, "minSize": 10, "maxSize": 5}),
        json!({"paths": [], "matchBy": {"content": true}}),
        json!({"paths": ["relative"], "matchBy": {"content": true}}),
        json!({"paths": [b.tree.join("nope")], "matchBy": {"content": true}}),
    ] {
        assert!(
            call(&b, "duplicates.scan", params.clone()).is_err(),
            "{params}"
        );
    }
}

#[test]
fn size_filters_hidden_system_and_excludes() {
    let b = bed();
    let t = &b.tree;
    put(&t.join("a/small1.txt"), b"tiny");
    put(&t.join("a/small2.txt"), b"tiny");
    put(&t.join("a/medium1.txt"), &[b'm'; 500]);
    put(&t.join("a/medium2.txt"), &[b'm'; 500]);
    put(&t.join("a/large1.txt"), &[b'l'; 5000]);
    put(&t.join("a/large2.txt"), &[b'l'; 5000]);
    let scan = |extra: Value| {
        let mut p = json!({"paths": [t], "matchBy": {"content": true}});
        for (k, val) in extra.as_object().unwrap() {
            p[k] = val.clone();
        }
        ok(&b, "duplicates.scan", p)
    };
    assert_eq!(scan(json!({}))["totalGroups"], 3);
    assert_eq!(scan(json!({"minSize": 100}))["totalGroups"], 2);
    assert_eq!(scan(json!({"maxSize": 1000}))["totalGroups"], 2);
    let mid = scan(json!({"minSize": 100, "maxSize": 1000}));
    assert_eq!(mid["totalGroups"], 1);
    assert_eq!(mid["groups"][0]["key"]["bytes"], 500);

    // excludePaths: a whole folder and a single file
    put(&t.join("ex/x1.txt"), b"tiny");
    let r = scan(json!({"excludePaths": [t.join("ex")]}));
    assert_eq!(r["scannedFiles"], 6);

    // hidden files and folders
    put(&t.join(".hidden/h1.txt"), b"hiddenhidden");
    put(&t.join("visible/h1.txt"), b"hiddenhidden");
    put(&t.join("a/.dot1"), b"dotdotdot");
    put(&t.join("a/.dot2"), b"dotdotdot");
    let plain = scan(json!({}));
    assert_eq!(plain["totalGroups"], 3, "hidden entries are ignored");
    let with_hidden = scan(json!({"includeHidden": true}));
    assert_eq!(with_hidden["totalGroups"], 5);
}

#[test]
fn system_folders_need_include_system() {
    let b = bed();
    if b.ctx.env.os != Os::Linux {
        return;
    }
    let usr = b.ctx.env.sys_path("/usr/share/x");
    put(&usr.join("a.txt"), b"system file");
    put(&usr.join("b.txt"), b"system file");
    let e = call(
        &b,
        "duplicates.scan",
        json!({"paths": [usr], "matchBy": {"content": true}}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    // inside an ordinary scan the system folder is skipped
    let base = &b.ctx.env.root;
    put(&base.join("home2/u.txt"), b"system file");
    let r = ok(
        &b,
        "duplicates.scan",
        json!({"paths": [base], "matchBy": {"content": true}}),
    );
    assert_eq!(r["totalGroups"], 0);
    let r = ok(
        &b,
        "duplicates.scan",
        json!({"paths": [base], "matchBy": {"content": true}, "includeSystem": true}),
    );
    assert_eq!(r["totalGroups"], 1);
    assert_eq!(r["totalFiles"], 3);
    // ... but such files can never be deleted
    let id = r["scanId"].as_str().unwrap();
    let del = ok(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [usr.join("a.txt")]}),
    );
    assert_eq!(del["results"][0]["ok"], false);
    assert!(usr.join("a.txt").exists());
}

#[test]
fn settings_exclusions_hide_files_from_the_search() {
    let b = bed();
    put(&b.tree.join("keep/a.txt"), b"same same");
    put(&b.tree.join("b.txt"), b"same same");
    settings::update(&b.ctx, |s| {
        s.exclude.push(settings::ExcludeEntry {
            id: "e".into(),
            pattern: b.tree.join("keep").to_string_lossy().into_owned(),
        });
    })
    .unwrap();
    assert_eq!(content_scan(&b)["totalGroups"], 0);
}

#[test]
fn scan_is_cancellable() {
    let b = bed();
    put(&b.tree.join("a.txt"), b"x");
    let token = CancelToken::new();
    token.cancel();
    let e = engine::find(
        &b.ctx,
        &[b.tree.canonicalize().unwrap()],
        &ExcludeSet::empty(),
        &Options {
            match_by: MatchBy {
                content: true,
                ..Default::default()
            },
            min_size: None,
            max_size: None,
            include_hidden: false,
            include_system: false,
            skip_zero_byte: true,
            follow_links: false,
            case_insensitive_names: false,
        },
        &Job::with_token(token),
    )
    .err()
    .unwrap();
    assert_eq!(e.code, ErrorCode::Cancelled);
}

#[test]
fn groups_paging() {
    let b = bed();
    for i in 0..5u8 {
        put(
            &b.tree.join(format!("p{i}/f.bin")),
            &vec![i; 100 + i as usize],
        );
        put(
            &b.tree.join(format!("q{i}/f.bin")),
            &vec![i; 100 + i as usize],
        );
    }
    let r = content_scan(&b);
    let id = r["scanId"].as_str().unwrap();
    assert_eq!(r["totalGroups"], 5);
    let page = ok(
        &b,
        "duplicates.groups",
        json!({"scanId": id, "offset": 3, "limit": 10}),
    );
    assert_eq!(page["groups"].as_array().unwrap().len(), 2);
    assert_eq!(page["totalGroups"], 5);
    assert_eq!(page["groups"][0]["groupId"], "g3");
}

// ---------------------------------------------------------------- auto select

fn file(path: &str, modified: i64) -> engine::DupFile {
    engine::DupFile {
        path: path.into(),
        bytes: 10,
        modified,
    }
}

fn group(files: Vec<engine::DupFile>) -> Group {
    Group {
        group_id: "g0".into(),
        key: Default::default(),
        wasted_bytes: 10 * (files.len() as u64 - 1),
        files,
    }
}

#[test]
fn auto_select_rules() {
    let g = group(vec![
        file("/a/old.txt", 100),
        file("/a/b/newest.txt", 300),
        file("/z.txt", 200),
    ]);
    assert_eq!(
        select_in_group(&g, Rule::KeepNewest, None),
        vec!["/a/old.txt", "/z.txt"]
    );
    assert_eq!(
        select_in_group(&g, Rule::KeepOldest, None),
        vec!["/a/b/newest.txt", "/z.txt"]
    );
    assert_eq!(
        select_in_group(&g, Rule::KeepShortestPath, None),
        vec!["/a/old.txt", "/a/b/newest.txt"]
    );
    assert_eq!(
        select_in_group(&g, Rule::KeepInFolder, Some(Path::new("/a"))),
        vec!["/z.txt"]
    );
    // nothing lives in that folder: leave the group alone
    assert!(select_in_group(&g, Rule::KeepInFolder, Some(Path::new("/nope"))).is_empty());
    // everything lives in that folder: nothing to remove by this rule
    assert!(select_in_group(&g, Rule::KeepInFolder, Some(Path::new("/"))).is_empty());
    // a sibling with a common prefix is not "in" the folder
    let g2 = group(vec![file("/data/x.txt", 1), file("/data2/x.txt", 2)]);
    assert_eq!(
        select_in_group(&g2, Rule::KeepInFolder, Some(Path::new("/data"))),
        vec!["/data2/x.txt"]
    );
}

#[test]
fn auto_select_ties_are_deterministic_and_never_select_all() {
    let same = |rule| {
        let g = group(vec![file("/b/x", 5), file("/a/x", 5), file("/c/xx", 5)]);
        let sel = select_in_group(&g, rule, None);
        assert_eq!(sel.len(), 2, "{rule:?}");
        assert!(sel.len() < g.files.len());
        sel
    };
    assert_eq!(same(Rule::KeepNewest), vec!["/b/x", "/c/xx"]); // keeps /a/x
    assert_eq!(same(Rule::KeepOldest), vec!["/b/x", "/c/xx"]);
    assert_eq!(same(Rule::KeepShortestPath), vec!["/b/x", "/c/xx"]);
    // exhaustive: every rule on groups of size 2..6 leaves at least one file
    for n in 2..7 {
        let files: Vec<_> = (0..n)
            .map(|i| file(&format!("/d{i}/f"), (i % 3) as i64))
            .collect();
        let g = group(files);
        for rule in [
            Rule::KeepNewest,
            Rule::KeepOldest,
            Rule::KeepShortestPath,
            Rule::KeepInFolder,
        ] {
            for folder in [None, Some(Path::new("/d0")), Some(Path::new("/"))] {
                let sel = select_in_group(&g, rule, folder);
                assert!(sel.len() < g.files.len(), "{rule:?} {n} {folder:?}");
            }
        }
    }
}

#[test]
fn auto_select_api() {
    let b = bed();
    put(&b.tree.join("old/f.txt"), b"same data");
    put(&b.tree.join("new/f.txt"), b"same data");
    set_mtime(&b.tree.join("old/f.txt"), 1_000_000_000);
    set_mtime(&b.tree.join("new/f.txt"), 1_700_000_000);
    let r = content_scan(&b);
    let id = r["scanId"].as_str().unwrap();
    let sel = ok(
        &b,
        "duplicates.auto_select",
        json!({"scanId": id, "rule": "keep_newest"}),
    );
    assert_eq!(sel["count"], 1);
    assert_eq!(sel["bytes"], 9);
    assert_eq!(sel["groups"][0]["groupId"], "g0");
    assert_eq!(
        sel["groups"][0]["selected"][0],
        b.tree.join("old/f.txt").to_string_lossy().as_ref()
    );
    let sel = ok(
        &b,
        "duplicates.auto_select",
        json!({"scanId": id, "rule": "keep_oldest"}),
    );
    assert_eq!(
        sel["groups"][0]["selected"][0],
        b.tree.join("new/f.txt").to_string_lossy().as_ref()
    );
    let sel = ok(
        &b,
        "duplicates.auto_select",
        json!({"scanId": id, "rule": "keep_in_folder", "folder": b.tree.join("new")}),
    );
    assert_eq!(
        sel["groups"][0]["selected"][0],
        b.tree.join("old/f.txt").to_string_lossy().as_ref()
    );
    let missing_folder = call(
        &b,
        "duplicates.auto_select",
        json!({"scanId": id, "rule": "keep_in_folder"}),
    )
    .unwrap_err();
    assert_eq!(missing_folder.code, ErrorCode::InvalidParams);
    let bad_rule = call(
        &b,
        "duplicates.auto_select",
        json!({"scanId": id, "rule": "delete_all"}),
    )
    .unwrap_err();
    assert_eq!(bad_rule.code, ErrorCode::InvalidParams);
}

// ---------------------------------------------------------------- delete

fn three_copies(b: &Bed) -> (String, Vec<PathBuf>) {
    let paths: Vec<PathBuf> = ["a/f.txt", "b/f.txt", "c/f.txt"]
        .iter()
        .map(|p| b.tree.join(p))
        .collect();
    for (i, p) in paths.iter().enumerate() {
        put(p, b"identical bytes");
        set_mtime(p, 1_600_000_000 + i as i64);
    }
    let r = content_scan(b);
    assert_eq!(r["totalGroups"], 1);
    (r["scanId"].as_str().unwrap().to_string(), paths)
}

#[test]
fn delete_removes_selected_copies_and_keeps_one() {
    let b = bed();
    let (id, paths) = three_copies(&b);
    let r = ok(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [paths[0], paths[1]]}),
    );
    assert_eq!(r["deleted"], 2);
    assert_eq!(r["freedBytes"], 30);
    assert!(!paths[0].exists() && !paths[1].exists());
    assert!(paths[2].exists());
    assert_eq!(r["remainingGroups"], 0, "a single copy is not a duplicate");
    let g = ok(&b, "duplicates.groups", json!({"scanId": id}));
    assert_eq!(g["totalGroups"], 0);
}

#[test]
fn delete_rejects_requests_that_would_remove_every_copy() {
    let b = bed();
    let (id, paths) = three_copies(&b);
    let e = call(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": paths}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    assert!(paths.iter().all(|p| p.exists()), "nothing was touched");
    // duplicates in the request do not help either
    let e = call(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [paths[0], paths[0], paths[1], paths[2]]}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    assert!(paths.iter().all(|p| p.exists()));
    // and a valid second request still works afterwards
    ok(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [paths[0]]}),
    );
    assert!(!paths[0].exists());
    // now two copies remain: selecting both is rejected, selecting one is fine
    let e = call(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [paths[1], paths[2]]}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
}

#[test]
fn delete_rejects_paths_outside_the_scan_without_deleting_anything() {
    let b = bed();
    let (id, paths) = three_copies(&b);
    let outsider = b.tree.parent().unwrap().join("outsider.txt");
    put(&outsider, b"identical bytes");
    let unrelated = b.tree.join("unrelated.txt");
    put(&unrelated, b"something else");
    for bad in [
        json!([paths[0], outsider]),
        json!([paths[0], unrelated]),
        json!([format!("{}/a/../a/f.txt", b.tree.display())]),
        json!(["relative/f.txt"]),
    ] {
        let e = call(&b, "duplicates.delete", json!({"scanId": id, "paths": bad})).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams, "{bad}");
    }
    assert!(paths.iter().all(|p| p.exists()) && outsider.exists() && unrelated.exists());
    let e = call(&b, "duplicates.delete", json!({"scanId": id, "paths": []})).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    let e = call(
        &b,
        "duplicates.delete",
        json!({"scanId": "nope", "paths": [paths[0]]}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
}

#[test]
fn delete_reverifies_a_victim_whose_content_changed() {
    let b = bed();
    let (id, paths) = three_copies(&b);
    // Same length and same mtime, different bytes: only the hash can tell.
    fs::write(&paths[0], b"DIFFERENT bytes").unwrap();
    set_mtime(&paths[0], 1_600_000_000);
    let r = ok(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [paths[0], paths[1]]}),
    );
    let res = r["results"].as_array().unwrap();
    let by_path = |p: &Path| {
        res.iter()
            .find(|x| x["path"] == p.to_string_lossy().as_ref())
            .unwrap()
    };
    assert_eq!(by_path(&paths[0])["ok"], false);
    assert!(by_path(&paths[0])["error"]
        .as_str()
        .unwrap()
        .contains("content differs"));
    assert_eq!(by_path(&paths[1])["ok"], true);
    assert!(paths[0].exists(), "the changed file is kept");
    assert!(!paths[1].exists());
    assert!(paths[2].exists());
}

#[test]
fn delete_refuses_when_the_kept_copy_has_changed_or_vanished() {
    let b = bed();
    let (id, paths) = three_copies(&b);
    // both copies that would remain are altered / gone: nothing may be deleted
    fs::write(&paths[2], b"altered content").unwrap();
    let r = ok(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [paths[0], paths[1]]}),
    );
    assert_eq!(r["deleted"], 0);
    assert!(paths[0].exists() && paths[1].exists());
    fs::remove_file(&paths[2]).unwrap();
    let r = ok(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [paths[0], paths[1]]}),
    );
    assert_eq!(r["deleted"], 0);
    assert!(paths[0].exists() && paths[1].exists());
}

#[test]
fn delete_refuses_a_victim_modified_after_the_scan() {
    let b = bed();
    let (id, paths) = three_copies(&b);
    fs::write(&paths[0], b"grew by a lot").unwrap();
    let r = ok(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [paths[0]]}),
    );
    assert_eq!(r["results"][0]["ok"], false);
    assert!(paths[0].exists());
}

#[cfg(unix)]
#[test]
fn delete_never_follows_a_link_swapped_in_after_the_scan() {
    use std::os::unix::fs::symlink;
    let b = bed();
    let (id, paths) = three_copies(&b);
    fs::remove_file(&paths[0]).unwrap();
    symlink(&paths[2], &paths[0]).unwrap();
    let r = ok(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [paths[0]]}),
    );
    assert_eq!(r["results"][0]["ok"], false);
    assert!(paths[2].exists());
}

#[test]
fn non_content_groups_are_verified_by_size_and_date() {
    let b = bed();
    put(&b.tree.join("a/x.dat"), b"12345");
    put(&b.tree.join("b/y.dat"), b"abcde");
    set_mtime(&b.tree.join("a/x.dat"), 1_000_000_000);
    set_mtime(&b.tree.join("b/y.dat"), 1_000_000_000);
    let r = ok(
        &b,
        "duplicates.scan",
        json!({"paths": [b.tree], "matchBy": {"size": true, "modified": true}}),
    );
    assert_eq!(r["totalGroups"], 1);
    let id = r["scanId"].as_str().unwrap();
    // the survivor changed: refuse
    fs::write(b.tree.join("b/y.dat"), b"different length").unwrap();
    let d = ok(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [b.tree.join("a/x.dat")]}),
    );
    assert_eq!(d["deleted"], 0);
    assert!(b.tree.join("a/x.dat").exists());
    // restore it and the deletion goes through
    fs::write(b.tree.join("b/y.dat"), b"abcde").unwrap();
    set_mtime(&b.tree.join("b/y.dat"), 1_000_000_000);
    let d = ok(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [b.tree.join("a/x.dat")]}),
    );
    assert_eq!(d["deleted"], 1);
    assert!(!b.tree.join("a/x.dat").exists());
}

#[test]
fn delete_uses_secure_deletion_when_enabled() {
    let b = bed();
    settings::update(&b.ctx, |s| {
        s.secure_delete.enabled = true;
        s.secure_delete.passes = 1;
    })
    .unwrap();
    let (id, paths) = three_copies(&b);
    let r = ok(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [paths[0]]}),
    );
    assert_eq!(r["results"][0]["ok"], true);
    // secure deletion leaves no renamed remnant behind
    assert_eq!(fs::read_dir(b.tree.join("a")).unwrap().count(), 0);
}

#[test]
fn protected_paths_are_never_deleted() {
    let b = bed();
    let ssh = b.ctx.env.home.join(".ssh");
    put(&ssh.join("key"), b"secret material");
    put(&b.tree.join("copy_of_key"), b"secret material");
    let r = ok(
        &b,
        "duplicates.scan",
        json!({"paths": [b.ctx.env.home, b.tree], "matchBy": {"content": true}, "includeHidden": true}),
    );
    assert_eq!(r["totalGroups"], 1);
    let id = r["scanId"].as_str().unwrap();
    let d = ok(
        &b,
        "duplicates.delete",
        json!({"scanId": id, "paths": [ssh.join("key")]}),
    );
    assert_eq!(d["results"][0]["ok"], false);
    assert!(ssh.join("key").exists());
}

// ---------------------------------------------------------------- export

#[test]
fn export_formats() {
    let b = bed();
    put(&b.tree.join("dir, with comma/f \"q\".txt"), b"same");
    put(&b.tree.join("plain/f.txt"), b"same");
    set_mtime(&b.tree.join("plain/f.txt"), 0);
    let r = content_scan(&b);
    let id = r["scanId"].as_str().unwrap();
    let csv = ok(
        &b,
        "duplicates.export",
        json!({"scanId": id, "format": "csv"}),
    );
    assert_eq!(csv["format"], "csv");
    assert_eq!(csv["filename"], "clearsweep-duplicates.csv");
    let text = csv["text"].as_str().unwrap();
    let lines: Vec<&str> = text.split("\r\n").collect();
    assert_eq!(lines[0], "group,wasted_bytes,path,bytes,modified");
    assert_eq!(
        lines.len(),
        4,
        "header + 2 rows + trailing newline: {text:?}"
    );
    assert!(text.contains(&format!(
        "\"{}/dir, with comma/f \"\"q\"\".txt\"",
        b.tree.display()
    )));
    assert!(text.contains(&format!(
        "1,4,{}/plain/f.txt,4,1970-01-01T00:00:00Z",
        b.tree.display()
    )));

    let txt = ok(
        &b,
        "duplicates.export",
        json!({"scanId": id, "format": "txt"}),
    );
    let t = txt["text"].as_str().unwrap();
    assert!(t.starts_with("ClearSweep duplicate report: 1 groups, 4 bytes wasted"));
    assert!(t.contains("Group 1: 2 files, 4 bytes wasted"));
    assert!(t.contains(&format!(
        "  {}/plain/f.txt  (4 bytes, modified 1970-01-01T00:00:00Z)",
        b.tree.display()
    )));

    let bad = call(
        &b,
        "duplicates.export",
        json!({"scanId": id, "format": "pdf"}),
    )
    .unwrap_err();
    assert_eq!(bad.code, ErrorCode::InvalidParams);
}

#[test]
fn csv_quotes_only_when_needed() {
    assert_eq!(csv_field("plain"), "plain");
    assert_eq!(csv_field("a,b"), "\"a,b\"");
    assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
}

#[test]
fn partial_hash_covers_head_and_tail_only() {
    let d = tempfile::tempdir().unwrap();
    let mk = |name: &str, data: &[u8]| {
        let p = d.path().join(name);
        fs::write(&p, data).unwrap();
        p
    };
    let a = mk("a", &big(1, 2, 3));
    let b = mk("b", &big(1, 9, 3));
    let (ha, full_a) = engine::partial_hash(&a, 28_192).unwrap();
    let (hb, _) = engine::partial_hash(&b, 28_192).unwrap();
    assert!(!full_a);
    assert_eq!(ha, hb, "the middle is not sampled");
    let small = mk("s", b"tiny file");
    let (hs, full) = engine::partial_hash(&small, 9).unwrap();
    assert!(full);
    assert_eq!(
        hs,
        engine::hash_file(&small, &Job::detached(), |_| {}).unwrap(),
        "for small files the partial hash is the full hash"
    );
    // boundary: exactly 2 * PARTIAL_BYTES is fully covered, one more byte is not
    let edge = mk("e", &vec![5u8; 8192]);
    assert!(engine::partial_hash(&edge, 8192).unwrap().1);
    let edge1 = mk("e1", &vec![5u8; 8193]);
    assert!(!engine::partial_hash(&edge1, 8193).unwrap().1);
}
