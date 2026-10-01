use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use super::*;
use crate::api::dispatch;
use crate::elevate::{powershell_decode, with_elevation};
use crate::error::ErrorCode;
use crate::features::registry_cleaner::backup::{
    new_backup_dir, write_manifest, Manifest, ManifestIssue, KIND_CONFIG, KIND_REGISTRY,
};
use crate::runner::MockRunner;

fn ctx(os: Os) -> (tempfile::TempDir, Ctx, MockRunner) {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let mut c = Ctx::test(d.path(), m.clone());
    c.env.os = os;
    fs::create_dir_all(&c.env.home).unwrap();
    (d, c, m)
}

fn call(c: &Ctx, method: &str, params: Value) -> Result<Value> {
    dispatch(c, method, params, &Job::detached())
}

fn calls(m: &MockRunner) -> Vec<String> {
    m.calls()
        .into_iter()
        .map(|(p, a)| format!("{p} {}", a.join(" ")))
        .collect()
}

const TIMESHIFT: &str = "Device : /dev/sda1\nNum     Name                 Tags  Description\n------------------------------------------------------------------------------\n0    >  2024-01-01_10-00-01  O     Old one\n1    >  2024-03-01_03-00-00  D     Newest\n";
const SNAPPER_CFG: &str = "Config | Subvolume\n-------+----------\nroot   | /\n";
const SNAPPER: &str = "# | Type | Pre # | Date | User | Cleanup | Description | Userdata\n--+------+-------+------+------+---------+-------------+---------\n0 | single | | | root | | current |\n1 | single | | 2024-01-01 10:00:00 | root | | first |\n2 | single | | 2024-02-01 10:00:00 | root | | second |\n";
const TMUTIL: &str = "Snapshots for volume group containing disk /:\ncom.apple.TimeMachine.2024-01-01-100000.local\ncom.apple.TimeMachine.2024-02-01-100000.local\n";
const WIN3: &str = r#"[{"SequenceNumber":1,"Description":"A","CreationTime":"20240101100000.000000-000","RestorePointType":0},{"SequenceNumber":2,"Description":"B","CreationTime":"20240201100000.000000-000","RestorePointType":12},{"SequenceNumber":3,"Description":"C","CreationTime":"20240301100000.000000-000","RestorePointType":12}]"#;

fn linux_with_tools() -> (tempfile::TempDir, Ctx, MockRunner) {
    let (d, c, m) = ctx(Os::Linux);
    m.on("timeshift", &["--list"], CmdOutput::ok(TIMESHIFT));
    m.on("snapper", &["list-configs"], CmdOutput::ok(SNAPPER_CFG));
    m.on(
        "snapper",
        &["--iso", "-c", "root", "list"],
        CmdOutput::ok(SNAPPER),
    );
    (d, c, m)
}

fn points(v: &Value) -> Vec<Value> {
    v["points"].as_array().unwrap().clone()
}

// ---------------------------------------------------------------- listing

#[test]
fn linux_lists_timeshift_and_snapper_and_protects_the_newest() {
    let (_d, c, _m) = linux_with_tools();
    let v = call(&c, "restore.list_points", json!({})).unwrap();
    assert_eq!(v["supported"], true);
    assert_eq!(v["tool"], "timeshift");
    assert_eq!(v["tools"], json!(["timeshift", "snapper"]));
    assert_eq!(v["canCreate"], true);
    assert_eq!(v["canDeleteOld"], false);
    assert_eq!(v["needsAdmin"], false);
    let p = points(&v);
    assert_eq!(p.len(), 4);
    let by = |id: &str| p.iter().find(|x| x["id"] == id).unwrap();
    assert_eq!(by("timeshift:2024-01-01_10-00-01")["deletable"], true);
    assert_eq!(by("timeshift:2024-01-01_10-00-01")["isNewest"], false);
    assert_eq!(by("timeshift:2024-03-01_03-00-00")["deletable"], false);
    assert_eq!(by("timeshift:2024-03-01_03-00-00")["isNewest"], true);
    assert_eq!(by("snapper:root:2")["isNewest"], true);
    assert_eq!(by("snapper:root:1")["deletable"], true);
    assert_eq!(by("snapper:root:1")["kind"], "snapper");
    assert_eq!(by("snapper:root:1")["description"], "first");
    assert_eq!(by("snapper:root:1")["createdAt"], "2024-01-01T10:00:00");
    assert!(p.iter().all(|x| x["restorable"] == false));
    assert!(!p.iter().any(|x| x["id"] == "snapper:root:0"));
}

#[test]
fn listing_needs_admin_then_works_when_authorized() {
    let (_d, c, m) = ctx(Os::Linux);
    m.on(
        "timeshift",
        &["--list"],
        CmdOutput::failed(1, "Timeshift needs to be run as root\n"),
    );
    let v = call(&c, "restore.list_points", json!({})).unwrap();
    assert_eq!(v["needsAdmin"], true);
    assert!(points(&v).is_empty());
    with_elevation(true, || {
        m.on("timeshift", &["--list"], CmdOutput::ok(TIMESHIFT));
        let v = call(&c, "restore.list_points", json!({"elevate": true})).unwrap();
        assert_eq!(v["needsAdmin"], false);
        assert_eq!(points(&v).len(), 2);
    });
}

#[test]
fn unelevated_elevation_request_goes_through_pkexec() {
    with_elevation(false, || {
        let (_d, c, m) = ctx(Os::Linux);
        m.with_program("pkexec");
        m.on("pkexec", &["timeshift", "--list"], CmdOutput::ok(TIMESHIFT));
        m.with_program("timeshift");
        let v = call(&c, "restore.list_points", json!({"elevate": true})).unwrap();
        assert_eq!(points(&v).len(), 2);
    });
}

#[test]
fn nothing_installed_is_an_explained_empty_state_but_backups_still_show() {
    let (_d, c, _m) = ctx(Os::Linux);
    let (_id, dir) = new_backup_dir(&c, KIND_CONFIG).unwrap();
    write_manifest(&dir, &Manifest::new(KIND_CONFIG, Os::Linux, vec![])).unwrap();
    let v = call(&c, "restore.list_points", json!({})).unwrap();
    assert_eq!(v["supported"], false);
    assert!(v["tool"].is_null());
    assert!(v["hint"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("timeshift"));
    assert!(v["hint"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("snapper"));
    assert_eq!(v["canCreate"], false);
    let p = points(&v);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0]["kind"], "clearsweep-backup");
    assert_eq!(p[0]["restorable"], true);
    assert_eq!(p[0]["deletable"], true);
    assert_eq!(p[0]["backupKind"], "config");
    // creating is refused with the same explanation
    let e = call(&c, "restore.create_point", json!({"description": "x"})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
}

#[test]
fn windows_lists_points_single_object_and_array() {
    with_elevation(true, || {
        let (_d, c, m) = ctx(Os::Windows);
        m.with_program("powershell");
        let args = ["-NoProfile", "-NonInteractive", "-Command", WIN_LIST_SCRIPT];
        m.on("powershell", &args, CmdOutput::ok(WIN3));
        let v = call(&c, "restore.list_points", json!({"elevate": true})).unwrap();
        assert_eq!(v["tool"], "windows");
        assert_eq!(v["canDeleteOld"], true);
        assert_eq!(v["canOpenSystemTool"], true);
        let p = points(&v);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0]["id"], "windows:3"); // newest first
        assert_eq!(p[0]["isNewest"], true);
        assert!(p
            .iter()
            .all(|x| x["deletable"] == false && x["kind"] == "windows-restore-point"));
        assert_eq!(p[2]["createdAt"], "2024-01-01T10:00:00Z");

        m.on("powershell", &args, CmdOutput::ok(r#"{"SequenceNumber":9,"Description":"Only","CreationTime":"20240101100000.000000-000","RestorePointType":0}"#));
        let v = call(&c, "restore.list_points", json!({"elevate": true})).unwrap();
        assert_eq!(points(&v).len(), 1);
        assert_eq!(points(&v)[0]["isNewest"], true);
    });
}

#[test]
fn windows_listing_without_admin_asks_for_it() {
    let (_d, c, m) = ctx(Os::Windows);
    m.on(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", WIN_LIST_SCRIPT],
        CmdOutput::failed(1, "Get-ComputerRestorePoint : Access is denied"),
    );
    let v = call(&c, "restore.list_points", json!({})).unwrap();
    assert_eq!(v["needsAdmin"], true);
}

#[test]
fn macos_lists_local_snapshots() {
    let (_d, c, m) = ctx(Os::MacOs);
    m.on(
        "tmutil",
        &["listlocalsnapshots", "/"],
        CmdOutput::ok(TMUTIL),
    );
    let v = call(&c, "restore.list_points", json!({})).unwrap();
    assert_eq!(v["tool"], "tmutil");
    assert_eq!(v["canOpenSystemTool"], true);
    let p = points(&v);
    assert_eq!(p.len(), 2);
    assert_eq!(p[0]["id"], "tmutil:2024-02-01-100000");
    assert_eq!(p[0]["isNewest"], true);
    assert_eq!(p[1]["deletable"], true);
}

#[test]
fn clearsweep_backups_of_every_kind_are_listed() {
    let (_d, c, _m) = ctx(Os::Windows);
    let (_i, d1) = new_backup_dir(&c, KIND_REGISTRY).unwrap();
    let issue = ManifestIssue {
        id: "a".into(),
        category: "c".into(),
        description: "d".into(),
        location: "l".into(),
        value: None,
    };
    let mut m1 = Manifest::new(KIND_REGISTRY, Os::Windows, vec![issue.clone(), issue]);
    m1.created_at = "2024-05-01T00:00:00Z".into();
    write_manifest(&d1, &m1).unwrap();
    let (_i, d2) = new_backup_dir(&c, KIND_CONFIG).unwrap();
    let mut m2 = Manifest::new(KIND_CONFIG, Os::Linux, vec![]); // other platform: not restorable here
    m2.created_at = "2024-04-01T00:00:00Z".into();
    write_manifest(&d2, &m2).unwrap();
    let base = backup::backups_dir(&c);
    fs::write(base.join("uninstall-1700000000.reg"), b"REGEDIT4").unwrap();
    fs::create_dir_all(base.join("drivers-1700000001/x")).unwrap();
    fs::write(base.join("drivers-1700000001/x/a.inf"), b"inf").unwrap();
    // an aborted attempt, a foreign folder and a symlink are hidden
    fs::create_dir_all(base.join("registry-1")).unwrap();
    fs::create_dir_all(base.join("random")).unwrap();
    let outside = c.env.home.join("outside");
    fs::create_dir_all(&outside).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, base.join("config-9")).unwrap();

    let v = call(&c, "restore.list_points", json!({})).unwrap();
    let p = points(&v);
    assert_eq!(p.len(), 4, "{p:?}");
    assert!(p
        .iter()
        .all(|x| x["kind"] == "clearsweep-backup" && x["deletable"] == true));
    let reg = p.iter().find(|x| x["backupKind"] == "registry").unwrap();
    assert_eq!(reg["description"], "Registry backup (2 items)");
    assert_eq!(reg["restorable"], true);
    assert!(reg["sizeBytes"].as_u64().unwrap() > 0);
    let cfg = p.iter().find(|x| x["backupKind"] == "config").unwrap();
    assert_eq!(cfg["restorable"], false);
    let un = p
        .iter()
        .find(|x| x["backupKind"] == "uninstall-entry")
        .unwrap();
    assert_eq!(un["createdAt"], "2023-11-14T22:13:20Z");
    assert_eq!(un["restorable"], true);
    assert_eq!(
        p.iter().find(|x| x["backupKind"] == "drivers").unwrap()["sizeBytes"],
        3
    );
    // newest first
    assert_eq!(p[0]["backupKind"], "registry");
}

// ---------------------------------------------------------------- ids

#[test]
fn point_ids_are_strictly_validated() {
    for ok in [
        "windows:12",
        "timeshift:2024-01-01_10-00-01",
        "snapper:root:5",
        "snapper:my-cfg.1:12",
        "tmutil:2024-01-01-100000",
        "clearsweep:config-1700000000",
        "clearsweep:uninstall-1700000000.reg",
    ] {
        assert!(parse_id(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "windows:",
        "windows:1;calc",
        "windows:-1",
        "timeshift:x",
        "timeshift:2024-01-01_10-00-01 --delete",
        "timeshift:../../etc",
        "snapper:root:0",
        "snapper:-c:1",
        "snapper:root:1;rm -rf /",
        "snapper:root",
        "snapper::1",
        "tmutil:2024-01-01-1000",
        "clearsweep:../settings.json",
        "clearsweep:settings.json",
        "clearsweep:",
        "other:1",
        "no-colon",
    ] {
        assert_eq!(
            parse_id(bad).unwrap_err().code,
            ErrorCode::InvalidParams,
            "{bad}"
        );
    }
}

// ---------------------------------------------------------------- delete

#[test]
fn the_newest_point_can_never_be_deleted() {
    with_elevation(true, || {
        let (_d, c, m) = linux_with_tools();
        m.with_program("timeshift");
        for id in ["timeshift:2024-03-01_03-00-00", "snapper:root:2"] {
            let e = call(&c, "restore.delete_point", json!({"id": id})).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidParams, "{id}");
            assert!(e.message.contains("most recent"));
        }
        assert!(
            !calls(&m)
                .iter()
                .any(|l| l.contains("--delete") || l.contains(" delete ")),
            "{:?}",
            calls(&m)
        );
        // macOS
        let (_d, c, m) = ctx(Os::MacOs);
        m.on(
            "tmutil",
            &["listlocalsnapshots", "/"],
            CmdOutput::ok(TMUTIL),
        );
        let e = call(
            &c,
            "restore.delete_point",
            json!({"id": "tmutil:2024-02-01-100000"}),
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams);
        assert!(!calls(&m).iter().any(|l| l.contains("deletelocalsnapshots")));
    });
}

#[test]
fn older_points_are_deleted_with_the_right_commands() {
    with_elevation(true, || {
        let (_d, c, m) = linux_with_tools();
        m.on(
            "timeshift",
            &[
                "--delete",
                "--snapshot",
                "2024-01-01_10-00-01",
                "--yes",
                "--scripted",
            ],
            CmdOutput::ok("done"),
        );
        m.on("snapper", &["-c", "root", "delete", "1"], CmdOutput::ok(""));
        let a = call(
            &c,
            "restore.delete_point",
            json!({"id": "timeshift:2024-01-01_10-00-01"}),
        )
        .unwrap();
        assert_eq!(a["ok"], true);
        let b = call(&c, "restore.delete_point", json!({"id": "snapper:root:1"})).unwrap();
        assert_eq!(b["ok"], true);
        let (_d, c, m) = ctx(Os::MacOs);
        m.on(
            "tmutil",
            &["listlocalsnapshots", "/"],
            CmdOutput::ok(TMUTIL),
        );
        m.on(
            "tmutil",
            &["deletelocalsnapshots", "2024-01-01-100000"],
            CmdOutput::failed(1, "not allowed"),
        );
        let t = call(
            &c,
            "restore.delete_point",
            json!({"id": "tmutil:2024-01-01-100000"}),
        )
        .unwrap();
        assert_eq!(t["ok"], false);
        assert!(t["message"].as_str().unwrap().contains("not allowed"));
    });
}

#[test]
fn unknown_or_malformed_points_are_not_deleted() {
    with_elevation(true, || {
        let (_d, c, m) = linux_with_tools();
        let e = call(
            &c,
            "restore.delete_point",
            json!({"id": "timeshift:2020-01-01_00-00-00"}),
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
        for bad in ["timeshift:x --delete", "snapper:root:1;x", "", "windows:1"] {
            assert!(call(&c, "restore.delete_point", json!({"id": bad})).is_err());
        }
        assert!(!calls(&m).iter().any(|l| l.contains("delete")));
        // an empty listing (tool failed) never deletes anything
        let (_d, c, m) = ctx(Os::Linux);
        m.on("timeshift", &["--list"], CmdOutput::failed(1, "oops"));
        let e = call(
            &c,
            "restore.delete_point",
            json!({"id": "timeshift:2024-01-01_10-00-01"}),
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
    });
}

#[test]
fn windows_cannot_delete_a_single_point() {
    let (_d, c, m) = ctx(Os::Windows);
    m.with_program("powershell");
    let e = call(&c, "restore.delete_point", json!({"id": "windows:2"})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    assert!(e.message.contains("bulk"));
    assert!(m.calls().is_empty());
}

#[test]
fn windows_delete_old_keeps_the_newest() {
    with_elevation(true, || {
        let (_d, c, m) = ctx(Os::Windows);
        m.on(
            "powershell",
            &["-NoProfile", "-NonInteractive", "-Command", WIN_LIST_SCRIPT],
            CmdOutput::ok(WIN3),
        );
        m.on_any_args("powershell", CmdOutput::ok("OK\nOK\n"));
        let r = call(&c, "restore.delete_old", json!({})).unwrap();
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["deleted"], 2);
        let del = m
            .calls()
            .into_iter()
            .find(|(_, a)| a.contains(&"-EncodedCommand".to_string()))
            .unwrap();
        let script = powershell_decode(del.1.last().unwrap()).unwrap();
        assert!(script.contains("-lt 2"), "at most n-1 deletions: {script}");
        assert!(script.contains("/for=C: /oldest /quiet"));
        assert!(!script.contains("/all"));

        // a failure part-way is reported
        let (_d, c, m) = ctx(Os::Windows);
        m.on(
            "powershell",
            &["-NoProfile", "-NonInteractive", "-Command", WIN_LIST_SCRIPT],
            CmdOutput::ok(WIN3),
        );
        m.on_any_args("powershell", CmdOutput::ok("OK\nFAIL 2\n"));
        let r = call(&c, "restore.delete_old", json!({})).unwrap();
        assert_eq!(r["ok"], false);
        assert_eq!(r["deleted"], 1);

        // a single point: nothing to do, nothing run
        let (_d, c, m) = ctx(Os::Windows);
        m.on("powershell", &["-NoProfile", "-NonInteractive", "-Command", WIN_LIST_SCRIPT], CmdOutput::ok(r#"{"SequenceNumber":1,"Description":"x","CreationTime":"20240101100000.000000-000","RestorePointType":0}"#));
        let r = call(&c, "restore.delete_old", json!({})).unwrap();
        assert_eq!(r["deleted"], 0);
        assert_eq!(calls(&m).len(), 1);
    });
    let (_d, c, _m) = ctx(Os::Linux);
    assert_eq!(
        call(&c, "restore.delete_old", json!({})).unwrap_err().code,
        ErrorCode::Unsupported
    );
}

#[test]
fn deleting_a_clearsweep_backup_uses_the_safe_deleter_and_keeps_other_data() {
    let (_d, c, _m) = ctx(Os::Linux);
    let (id, dir) = new_backup_dir(&c, KIND_CONFIG).unwrap();
    write_manifest(&dir, &Manifest::new(KIND_CONFIG, Os::Linux, vec![])).unwrap();
    let file = backup::backups_dir(&c).join("uninstall-1700000000.reg");
    fs::write(&file, b"x").unwrap();
    fs::write(c.env.data_dir.join("settings.json"), b"{}").unwrap();
    let r = call(
        &c,
        "restore.delete_point",
        json!({"id": format!("clearsweep:{id}")}),
    )
    .unwrap();
    assert_eq!(r["ok"], true);
    assert!(!dir.exists());
    call(
        &c,
        "restore.delete_point",
        json!({"id": "clearsweep:uninstall-1700000000.reg"}),
    )
    .unwrap();
    assert!(!file.exists());
    assert!(c.env.data_dir.join("settings.json").exists());
    assert_eq!(
        call(
            &c,
            "restore.delete_point",
            json!({"id": "clearsweep:config-42"})
        )
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
}

// ---------------------------------------------------------------- create

#[test]
fn create_uses_each_tools_command() {
    with_elevation(true, || {
        let (_d, c, m) = ctx(Os::Linux);
        m.on(
            "timeshift",
            &[
                "--create",
                "--comments",
                "Before update",
                "--tags",
                "O",
                "--scripted",
            ],
            CmdOutput::ok("Snapshot saved"),
        );
        let r = call(
            &c,
            "restore.create_point",
            json!({"description": "  Before update "}),
        )
        .unwrap();
        assert_eq!(r["ok"], true);

        let (_d, c, m) = ctx(Os::Linux);
        m.with_program("snapper");
        m.on(
            "snapper",
            &[
                "-c",
                "root",
                "create",
                "--type",
                "single",
                "--description",
                "Before update",
            ],
            CmdOutput::ok("5"),
        );
        assert_eq!(
            call(
                &c,
                "restore.create_point",
                json!({"description": "Before update"})
            )
            .unwrap()["ok"],
            true
        );

        let (_d, c, m) = ctx(Os::MacOs);
        m.on(
            "tmutil",
            &["localsnapshot"],
            CmdOutput::ok("Created local snapshot"),
        );
        assert_eq!(
            call(&c, "restore.create_point", json!({"description": "x"})).unwrap()["ok"],
            true
        );

        let (_d, c, m) = ctx(Os::Linux);
        m.on(
            "timeshift",
            &["--create", "--comments", "x", "--tags", "O", "--scripted"],
            CmdOutput::failed(1, "No snapshot device"),
        );
        let r = call(&c, "restore.create_point", json!({"description": "x"})).unwrap();
        assert_eq!(r["ok"], false);
        assert!(r["message"]
            .as_str()
            .unwrap()
            .contains("No snapshot device"));
    });
}

#[test]
fn windows_create_reports_success_throttling_and_errors() {
    with_elevation(true, || {
        for (out, ok, throttled) in [
            ("CREATED\r\n", true, false),
            ("WARN: A new system restore point cannot be created because one has already been created within the past 1440 minutes.\n", false, true),
            ("ERROR: System Restore is disabled.\n", false, false),
        ] {
            let (_d, c, m) = ctx(Os::Windows);
            m.on_any_args("powershell", CmdOutput::ok(out));
            let r = call(&c, "restore.create_point", json!({"description": "It's a test"})).unwrap();
            assert_eq!(r["ok"], ok, "{out}");
            assert_eq!(r["throttled"], throttled, "{out}");
            if throttled {
                assert!(r["message"].as_str().unwrap().contains("24 hours"));
            }
            let script = powershell_decode(m.calls()[0].1.last().unwrap()).unwrap();
            assert!(script.contains("Checkpoint-Computer -Description 'It''s a test' -RestorePointType MODIFY_SETTINGS"));
        }
        let (_d, c, m) = ctx(Os::Windows);
        m.on_any_args(
            "powershell",
            CmdOutput::ok("ERROR: System Restore is disabled.\n"),
        );
        let r = call(&c, "restore.create_point", json!({"description": "x"})).unwrap();
        assert!(r["message"]
            .as_str()
            .unwrap()
            .contains("System Restore is disabled"));
    });
}

#[test]
fn descriptions_are_validated() {
    let (_d, c, m) = ctx(Os::Linux);
    m.with_program("timeshift");
    for bad in ["", "   ", "-rm", "a\nb", &"x".repeat(201)] {
        let e = call(&c, "restore.create_point", json!({"description": bad})).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams, "{bad:?}");
    }
    assert!(call(&c, "restore.create_point", json!({})).is_err());
    assert!(m.calls().is_empty());
}

// ---------------------------------------------------------------- restore

#[test]
fn operating_system_points_are_not_rolled_back_by_clearsweep() {
    let (_d, c, m) = linux_with_tools();
    for id in [
        "windows:3",
        "timeshift:2024-01-01_10-00-01",
        "snapper:root:1",
        "tmutil:2024-01-01-100000",
    ] {
        let e = call(&c, "restore.restore", json!({"id": id})).unwrap_err();
        assert_eq!(e.code, ErrorCode::Unsupported, "{id}");
        assert!(e.message.len() > 40);
    }
    assert!(m.calls().is_empty());
}

#[cfg(unix)]
#[test]
fn config_backups_are_restored_through_the_owning_feature() {
    use crate::features::registry_cleaner::backup::capture_path;
    let (_d, c, _m) = ctx(Os::Linux);
    let (id, dir) = new_backup_dir(&c, KIND_CONFIG).unwrap();
    let f = c.env.home.join(".config/autostart/a.desktop");
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    fs::write(&f, b"[Desktop Entry]\n").unwrap();
    let mut m = Manifest::new(KIND_CONFIG, Os::Linux, vec![]);
    m.files.push(capture_path(&c, &dir, 0, &f).unwrap());
    write_manifest(&dir, &m).unwrap();
    fs::remove_file(&f).unwrap();
    let r = call(
        &c,
        "restore.restore",
        json!({"id": format!("clearsweep:{id}")}),
    )
    .unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(fs::read(&f).unwrap(), b"[Desktop Entry]\n");
    assert!(r["message"].as_str().unwrap().contains("Restored 1 item"));
    assert_eq!(
        call(&c, "restore.restore", json!({"id": "clearsweep:config-99"}))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

fn utf16(s: &str) -> Vec<u8> {
    let mut v = vec![0xFF, 0xFE];
    v.extend(s.encode_utf16().flat_map(|u| u.to_le_bytes()));
    v
}

#[test]
fn uninstall_entry_backups_import_with_the_right_rights() {
    with_elevation(true, || {
        let (_d, c, m) = ctx(Os::Windows);
        let base = backup::backups_dir(&c);
        fs::create_dir_all(&base).unwrap();
        fs::write(
            base.join("uninstall-1.reg"),
            utf16(
                "Windows Registry Editor Version 5.00\r\n\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\X]\r\n",
            ),
        )
        .unwrap();
        fs::write(
            base.join("uninstall-2.reg"),
            utf16(
                "Windows Registry Editor Version 5.00\r\n\r\n[HKEY_CURRENT_USER\\SOFTWARE\\X]\r\n",
            ),
        )
        .unwrap();
        m.on_any_args("powershell", CmdOutput::ok("R0:0:\n"));
        m.on_any_args("reg", CmdOutput::ok(""));
        let a = call(
            &c,
            "restore.restore",
            json!({"id": "clearsweep:uninstall-1.reg"}),
        )
        .unwrap();
        assert_eq!(a["ok"], true, "{a}");
        assert!(calls(&m)[0].starts_with("powershell"));
        let b = call(
            &c,
            "restore.restore",
            json!({"id": "clearsweep:uninstall-2.reg"}),
        )
        .unwrap();
        assert_eq!(b["ok"], true);
        assert!(calls(&m)[1].starts_with("reg import "));
        // not on Linux
        let (_d, c, _m) = ctx(Os::Linux);
        let base = backup::backups_dir(&c);
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("uninstall-1.reg"), b"x").unwrap();
        assert_eq!(
            call(
                &c,
                "restore.restore",
                json!({"id": "clearsweep:uninstall-1.reg"})
            )
            .unwrap_err()
            .code,
            ErrorCode::Unsupported
        );
    });
}

#[test]
fn driver_backups_reinstall_with_pnputil() {
    with_elevation(true, || {
        let (_d, c, m) = ctx(Os::Windows);
        let dir = backup::backups_dir(&c).join("drivers-1700000000");
        fs::create_dir_all(&dir).unwrap();
        m.on_any_args("pnputil", CmdOutput::ok("Total attempted: 3"));
        let r = call(
            &c,
            "restore.restore",
            json!({"id": "clearsweep:drivers-1700000000"}),
        )
        .unwrap();
        assert_eq!(r["ok"], true);
        let (_, a) = &m.calls()[0];
        assert_eq!(a[0], "/add-driver");
        assert!(a[1].ends_with("drivers-1700000000\\*.inf"));
        assert_eq!(&a[2..], ["/subdirs", "/install"]);
        let (_d, c, _m) = ctx(Os::Linux);
        fs::create_dir_all(backup::backups_dir(&c).join("drivers-1700000000")).unwrap();
        assert_eq!(
            call(
                &c,
                "restore.restore",
                json!({"id": "clearsweep:drivers-1700000000"})
            )
            .unwrap_err()
            .code,
            ErrorCode::Unsupported
        );
    });
}

// ---------------------------------------------------------------- system tool

#[test]
fn open_system_tool_per_platform() {
    let (_d, c, m) = ctx(Os::Windows);
    m.with_program("powershell");
    m.on("cmd", &["/c", "start", "", "rstrui.exe"], CmdOutput::ok(""));
    assert_eq!(
        call(&c, "restore.open_system_tool", json!({})).unwrap()["ok"],
        true
    );

    let (_d, c, m) = ctx(Os::Linux);
    m.with_program("timeshift-launcher");
    m.on(
        "sh",
        &["-c", "nohup timeshift-launcher >/dev/null 2>&1 &"],
        CmdOutput::ok(""),
    );
    assert_eq!(
        call(&c, "restore.open_system_tool", json!({})).unwrap()["ok"],
        true
    );

    let (_d, c, m) = ctx(Os::MacOs);
    m.with_program("tmutil");
    m.on("open", &["-a", "Time Machine"], CmdOutput::ok(""));
    assert_eq!(
        call(&c, "restore.open_system_tool", json!({})).unwrap()["ok"],
        true
    );

    // snapper only: there is no GUI
    let (_d, c, m) = ctx(Os::Linux);
    m.with_program("snapper");
    assert_eq!(
        call(&c, "restore.open_system_tool", json!({}))
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
    let (_d, c, _m) = ctx(Os::Windows);
    assert_eq!(
        call(&c, "restore.open_system_tool", json!({}))
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
}

#[test]
fn helper_functions() {
    assert_eq!(
        system_drive(&{
            let (_d, mut c, _m) = ctx(Os::Windows);
            c.env.root = Path::new("d:\\").to_path_buf();
            c
        }),
        "D:"
    );
    assert_eq!(ts_of("drivers-1700000000"), Some(1_700_000_000));
    assert_eq!(ts_of("uninstall-1700000000-2.reg"), Some(1_700_000_000));
    assert!(reg_file_needs_admin(&reg_file_text(&utf16(
        "[HKEY_CLASSES_ROOT\\x]"
    ))));
    assert!(!reg_file_needs_admin("[HKEY_CURRENT_USER\\x]"));
    assert!(windows_create_script("a").contains("Checkpoint-Computer"));
}

// ---------------------------------------------------------------- startup + add-on backups

const SLACK_DESKTOP: &str =
    "[Desktop Entry]\nType=Application\nName=Slack\nExec=/usr/bin/slack --startup\n";

fn slack_ctx() -> (tempfile::TempDir, Ctx, std::path::PathBuf) {
    let (d, c, _m) = ctx(Os::Linux);
    let f = crate::features::startup::linux::user_autostart_dir(&c).join("slack.desktop");
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    fs::write(&f, SLACK_DESKTOP).unwrap();
    (d, c, f)
}

#[test]
fn removed_startup_items_are_listed_and_restored_through_the_restore_feature() {
    let (_d, c, f) = slack_ctx();
    let r = call(
        &c,
        "startup.remove",
        json!({"id": "xdg:user:slack.desktop"}),
    )
    .unwrap();
    let bid = r["backupId"].as_str().unwrap().to_string();
    assert!(!f.exists());

    let v = call(&c, "restore.list_points", json!({})).unwrap();
    let pt = points(&v)
        .into_iter()
        .find(|p| p["id"] == format!("clearsweep:{bid}"))
        .unwrap_or_else(|| panic!("startup backup not listed: {v}"));
    assert_eq!(pt["description"], "Removed startup item: Slack");
    assert_eq!(pt["kind"], "clearsweep-backup");
    assert_eq!(pt["backupKind"], "startup");
    assert_eq!(pt["restorable"], true);
    assert_eq!(pt["deletable"], true);
    assert!(pt["createdAt"].as_str().unwrap().ends_with('Z'));

    let out = call(
        &c,
        "restore.restore",
        json!({"id": format!("clearsweep:{bid}")}),
    )
    .unwrap();
    assert_eq!(out["ok"], true);
    assert_eq!(out["restored"], 1);
    assert_eq!(out["message"], "Restored startup item Slack.");
    assert_eq!(fs::read_to_string(&f).unwrap(), SLACK_DESKTOP);

    // A second restore leaves the existing file alone and says so.
    fs::write(
        &f,
        "[Desktop Entry]\nType=Application\nName=Slack\nExec=newer\n",
    )
    .unwrap();
    let out = call(
        &c,
        "restore.restore",
        json!({"id": format!("clearsweep:{bid}")}),
    )
    .unwrap();
    assert_eq!(out["restored"], 0);
    assert!(out["message"].as_str().unwrap().contains("already exists"));
    assert!(fs::read_to_string(&f).unwrap().contains("Exec=newer"));

    // And it can be deleted like any other backup.
    let del = call(
        &c,
        "restore.delete_point",
        json!({"id": format!("clearsweep:{bid}")}),
    )
    .unwrap();
    assert_eq!(del["ok"], true);
    assert!(!c.env.data_dir.join("backups").join(&bid).exists());
}

#[test]
fn restore_of_a_startup_backup_revalidates_the_manifest_server_side() {
    let (_d, c, f) = slack_ctx();
    let r = call(
        &c,
        "startup.remove",
        json!({"id": "xdg:user:slack.desktop"}),
    )
    .unwrap();
    let bid = r["backupId"].as_str().unwrap().to_string();
    let mpath = c
        .env
        .data_dir
        .join("backups")
        .join(&bid)
        .join("manifest.json");
    let mut m: Value = serde_json::from_str(&fs::read_to_string(&mpath).unwrap()).unwrap();
    // A forged target outside the startup folders is refused, exactly as startup refuses it.
    m["items"][0]["original"] = json!(c.env.home.join(".bashrc").to_string_lossy());
    fs::write(&mpath, m.to_string()).unwrap();
    let e = call(
        &c,
        "restore.restore",
        json!({"id": format!("clearsweep:{bid}")}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(!c.env.home.join(".bashrc").exists());
    assert!(!f.exists());
    // A damaged manifest fails cleanly.
    fs::write(&mpath, "{ not json").unwrap();
    let e = call(
        &c,
        "restore.restore",
        json!({"id": format!("clearsweep:{bid}")}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Io);
    // Names that only look like backups are not backups.
    for bad in [
        "clearsweep:startup-1/../x",
        "clearsweep:startup-",
        "clearsweep:startup-1-2-3",
        "clearsweep:plugins-x",
    ] {
        let e = call(&c, "restore.restore", json!({"id": bad})).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams, "{bad}");
    }
    // Missing backup.
    let e = call(
        &c,
        "restore.restore",
        json!({"id": "clearsweep:startup-1700000000"}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
}

#[test]
fn startup_backups_that_cannot_be_applied_here_are_not_offered_as_restorable() {
    let (_d, c, _f) = slack_ctx();
    let r = call(
        &c,
        "startup.remove",
        json!({"id": "xdg:user:slack.desktop"}),
    )
    .unwrap();
    let bid = r["backupId"].as_str().unwrap().to_string();
    let mpath = c
        .env
        .data_dir
        .join("backups")
        .join(&bid)
        .join("manifest.json");
    let mut m: Value = serde_json::from_str(&fs::read_to_string(&mpath).unwrap()).unwrap();
    m["items"] = json!([{"type": "regfile", "backup": "files/1-x.reg", "admin": false}]);
    fs::write(&mpath, m.to_string()).unwrap();
    let v = call(&c, "restore.list_points", json!({})).unwrap();
    let pt = points(&v)
        .into_iter()
        .find(|p| p["id"] == format!("clearsweep:{bid}"))
        .unwrap();
    assert_eq!(pt["restorable"], false);
    // A backup folder without a manifest (an aborted attempt) is not listed at all.
    fs::remove_file(&mpath).unwrap();
    let v = call(&c, "restore.list_points", json!({})).unwrap();
    assert!(points(&v)
        .iter()
        .all(|p| p["id"] != format!("clearsweep:{bid}")));
}
