//! Scan -> fix -> backup -> restore through the public API, on the fake Windows registry and on
//! fixture trees. External programs are a scripted `MockRunner` wrapped in [`FsRunner`], which
//! also performs the few file effects a real `reg export`, `rm`, `install` and `ln` would have.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use super::fake::FakeWin;
use super::regaccess::Root;
use super::with_registry;
use crate::api::dispatch;
use crate::ctx::{Ctx, Os};
use crate::elevate::with_elevation;
use crate::error::{ApiError, ErrorCode, Result};
use crate::job::Job;
use crate::runner::{CmdOutput, CommandRunner, MockRunner};

// ---------------------------------------------------------------- harness

/// `MockRunner` plus the file effects of the commands the cleaner relies on.
struct FsRunner {
    mock: MockRunner,
    /// `reg export` of a key containing this text fails.
    fail_export: Mutex<Option<String>>,
    /// `reg export` reports success but writes nothing.
    silent_export: Mutex<bool>,
}

impl CommandRunner for FsRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput> {
        let scripted = self.mock.run(program, args); // records the call
        match (program, args) {
            ("reg", ["export", key, file, ..]) => {
                if self.fail_export.lock().unwrap().as_deref().is_some_and(|f| key.contains(f)) {
                    return Ok(CmdOutput::failed(1, "ERROR: Access is denied."));
                }
                if !*self.silent_export.lock().unwrap() {
                    fs::write(file, format!("Windows Registry Editor Version 5.00\r\n\r\n[{key}]\r\n"))?;
                }
                Ok(CmdOutput::ok(""))
            }
            ("rm", ["-f", "--", paths @ ..]) => {
                for p in paths {
                    let _ = fs::remove_file(p);
                }
                Ok(CmdOutput::ok(""))
            }
            ("pkexec", [prog, rest @ ..]) => self.run(prog, rest),
            ("install", ["-m", mode, src, dst]) => {
                fs::copy(src, dst)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let m = u32::from_str_radix(mode, 8).unwrap();
                    fs::set_permissions(dst, fs::Permissions::from_mode(m))?;
                }
                let _ = mode;
                Ok(CmdOutput::ok(""))
            }
            ("ln", ["-sfn", "--", target, dst]) => {
                let _ = fs::remove_file(dst);
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, dst)?;
                let _ = target;
                Ok(CmdOutput::ok(""))
            }
            _ => scripted,
        }
    }
    fn which(&self, program: &str) -> Option<PathBuf> {
        self.mock.which(program)
    }
}

struct H {
    _d: tempfile::TempDir,
    ctx: Ctx,
    mock: MockRunner,
    runner: Arc<FsRunner>,
}

fn harness(os: Os) -> H {
    let d = tempfile::tempdir().unwrap();
    let mock = MockRunner::new();
    let runner = Arc::new(FsRunner {
        mock: mock.clone(),
        fail_export: Mutex::new(None),
        silent_export: Mutex::new(false),
    });
    let mut env = crate::ctx::Env::for_test(d.path());
    env.os = os;
    fs::create_dir_all(&env.home).unwrap();
    fs::create_dir_all(&env.root).unwrap();
    let ctx = Ctx::new(env, runner.clone());
    H {
        _d: d,
        ctx,
        mock,
        runner,
    }
}

fn call(c: &Ctx, method: &str, params: Value) -> Result<Value> {
    dispatch(c, method, params, &Job::detached())
}

fn ids(scan: &Value) -> Vec<String> {
    scan["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect()
}

fn calls(m: &MockRunner) -> Vec<String> {
    m.calls()
        .into_iter()
        .map(|(p, a)| format!("{p} {}", a.join(" ")))
        .collect()
}

// ---------------------------------------------------------------- Windows

const SHARED: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\SharedDLLs";
const RUN: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const MUI: &str = r"SOFTWARE\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache";

fn windows_machine() -> Rc<FakeWin> {
    let w = Rc::new(FakeWin::new());
    w.file(r"C:\Program Files\Common Files\ok.dll");
    w.dword(Root::Hklm, SHARED, r"C:\Program Files\Common Files\ok.dll", 1);
    w.dword(Root::Hklm, SHARED, r"C:\Program Files\Common Files\gone1.dll", 1);
    w.dword(Root::Hklm, SHARED, r"C:\Program Files\Common Files\gone2.dll", 2);
    w.sz(Root::Hkcu, RUN, "Gone", r#""C:\Program Files\Gone\gone.exe" --tray"#);
    w.sz(Root::Hkcu, MUI, r"C:\Program Files\Gone\gone.exe.FriendlyAppName", "Gone");
    w
}

fn windows_scan(h: &H, w: &Rc<FakeWin>) -> Value {
    with_registry(w.clone(), || call(&h.ctx, "registry_cleaner.scan", json!({})).unwrap())
}

#[test]
fn windows_scan_reports_counts_for_every_category_and_leaves_the_machine_alone() {
    let h = harness(Os::Windows);
    let w = windows_machine();
    let s = windows_scan(&h, &w);
    assert_eq!(s["platform"], "windows");
    assert_eq!(s["title"], "Registry");
    assert_eq!(s["issues"].as_array().unwrap().len(), 4);
    assert_eq!(s["counts"]["shared_dlls"], 2);
    assert_eq!(s["counts"]["startup"], 1);
    assert_eq!(s["counts"]["mui_cache"], 1);
    assert_eq!(s["counts"]["fonts"], 0);
    assert_eq!(s["scanned"].as_array().unwrap().len(), 15);
    assert!(s["skipped"].as_array().unwrap().is_empty());
    let first = &s["issues"][0];
    for k in ["id", "category", "description", "location", "severity", "needsAdmin"] {
        assert!(first.get(k).is_some(), "{k}");
    }
    assert_eq!(first["id"].as_str().unwrap().len(), 32);
    // scanning never runs a program
    assert!(h.mock.calls().is_empty());
    // ids are stable across scans
    assert_eq!(ids(&s), ids(&windows_scan(&h, &w)));
    // a category subset
    let sub = with_registry(w.clone(), || {
        call(&h.ctx, "registry_cleaner.scan", json!({"categories": ["startup"]})).unwrap()
    });
    assert_eq!(sub["issues"].as_array().unwrap().len(), 1);
    assert_eq!(sub["scanned"], json!(["startup"]));
    // unknown / empty category lists are rejected
    with_registry(w.clone(), || {
        for bad in [json!({"categories": ["nope"]}), json!({"categories": []})] {
            assert_eq!(call(&h.ctx, "registry_cleaner.scan", bad).unwrap_err().code, ErrorCode::InvalidParams);
        }
    });
}

#[test]
fn windows_scan_without_a_registry_is_unsupported_off_windows() {
    let h = harness(Os::Windows);
    let e = call(&h.ctx, "registry_cleaner.scan", json!({})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
}

#[test]
fn categories_are_platform_aware() {
    for (os, title, n) in [(Os::Windows, "Registry", 15), (Os::Linux, "Config Issues", 6), (Os::MacOs, "Config Issues", 2)] {
        let h = harness(os);
        let v = call(&h.ctx, "registry_cleaner.categories", json!({})).unwrap();
        assert_eq!(v["title"], title);
        assert_eq!(v["categories"].as_array().unwrap().len(), n);
        assert!(v["categories"][0].get("defaultSelected").is_some());
    }
}

#[test]
fn windows_fix_backs_up_every_key_before_the_first_delete() {
    with_elevation(true, || {
        let h = harness(Os::Windows);
        let w = windows_machine();
        h.mock.on_any_args("reg", CmdOutput::ok(""));
        h.mock.on_any_args("powershell", CmdOutput::ok("R0:0:\nR1:0:\n"));
        let scan = windows_scan(&h, &w);
        let all = ids(&scan);
        let out = with_registry(w.clone(), || {
            call(&h.ctx, "registry_cleaner.fix", json!({"issueIds": all, "backup": true})).unwrap()
        });
        assert_eq!(out["fixed"], 4, "{out}");
        assert_eq!(out["failed"], 0);
        let backup_id = out["backupId"].as_str().unwrap().to_string();

        let c = calls(&h.mock);
        let first_change = c.iter().position(|l| l.starts_with("reg delete") || l.starts_with("powershell")).unwrap();
        let exports: Vec<usize> = c.iter().enumerate().filter(|(_, l)| l.starts_with("reg export")).map(|(i, _)| i).collect();
        // three distinct keys, all exported before anything changes
        assert_eq!(exports.len(), 3, "{c:?}");
        assert!(exports.iter().all(|i| *i < first_change), "{c:?}");
        // per-user changes are direct, machine-wide ones are one elevated batch
        assert_eq!(c.iter().filter(|l| l.starts_with("reg delete")).count(), 2);
        assert_eq!(c.iter().filter(|l| l.starts_with("powershell")).count(), 1);
        let ps = &h.mock.calls().into_iter().find(|(p, _)| p == "powershell").unwrap().1;
        assert!(ps[3].contains("gone1.dll") && ps[3].contains("gone2.dll"));
        assert!(ps[3].contains(r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\SharedDLLs"));
        assert!(c.iter().any(|l| l == r#"reg delete HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Run /v Gone /f"#), "{c:?}");

        // the backup exists and is listed
        let dir = h.ctx.env.data_dir.join("backups").join(&backup_id);
        assert!(dir.join("manifest.json").is_file());
        assert!(dir.join("000.reg").is_file());
        let list = call(&h.ctx, "registry_cleaner.list_backups", json!({})).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["id"], backup_id.as_str());
        assert_eq!(list[0]["issueCount"], 4);
        assert_eq!(list[0]["platform"], "windows");
        assert!(list[0]["sizeBytes"].as_u64().unwrap() > 0);
        assert!(list[0]["createdAt"].as_str().unwrap().contains('T'));
    });
}

#[test]
fn ids_that_are_not_in_a_fresh_scan_are_refused_and_change_nothing() {
    with_elevation(true, || {
        let h = harness(Os::Windows);
        let w = windows_machine();
        let real = ids(&windows_scan(&h, &w))[0].clone();
        let out = with_registry(w.clone(), || {
            call(
                &h.ctx,
                "registry_cleaner.fix",
                json!({"issueIds": ["deadbeef", "../../etc/passwd", format!("{real}x")], "backup": true}),
            )
            .unwrap()
        });
        assert_eq!(out["fixed"], 0);
        assert_eq!(out["failed"], 3);
        assert!(out["backupId"].is_null());
        assert!(out["results"][0]["error"].as_str().unwrap().contains("fresh scan"));
        assert!(h.mock.calls().is_empty(), "no program may run: {:?}", calls(&h.mock));
        assert!(!h.ctx.env.data_dir.join("backups").exists());
    });
}

#[test]
fn only_the_requested_ids_are_touched() {
    with_elevation(true, || {
        let h = harness(Os::Windows);
        let w = windows_machine();
        h.mock.on_any_args("reg", CmdOutput::ok(""));
        let scan = windows_scan(&h, &w);
        let startup = scan["issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["category"] == "startup")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let out = with_registry(w.clone(), || {
            call(&h.ctx, "registry_cleaner.fix", json!({"issueIds": [startup, startup]})).unwrap()
        });
        assert_eq!(out["fixed"], 1);
        let c = calls(&h.mock);
        assert_eq!(c.iter().filter(|l| l.starts_with("reg export")).count(), 1);
        assert_eq!(c.iter().filter(|l| l.starts_with("reg delete")).count(), 1);
        assert!(!c.iter().any(|l| l.starts_with("powershell")));
    });
}

#[test]
fn a_failed_backup_aborts_before_any_change() {
    with_elevation(true, || {
        let h = harness(Os::Windows);
        let w = windows_machine();
        h.mock.on_any_args("reg", CmdOutput::ok(""));
        *h.runner.fail_export.lock().unwrap() = Some("MuiCache".into());
        let all = ids(&windows_scan(&h, &w));
        let e = with_registry(w.clone(), || {
            call(&h.ctx, "registry_cleaner.fix", json!({"issueIds": all, "backup": true})).unwrap_err()
        });
        assert_eq!(e.code, ErrorCode::Io);
        assert!(e.message.contains("nothing was changed"), "{}", e.message);
        let c = calls(&h.mock);
        assert!(!c.iter().any(|l| l.starts_with("reg delete") || l.starts_with("powershell")), "{c:?}");
        // the half-made backup does not linger
        let backups = h.ctx.env.data_dir.join("backups");
        assert!(fs::read_dir(&backups).map(|mut r| r.next().is_none()).unwrap_or(true));
        assert!(call(&h.ctx, "registry_cleaner.list_backups", json!({})).unwrap().as_array().unwrap().is_empty());
    });
}

#[test]
fn an_export_that_writes_nothing_is_not_a_backup() {
    with_elevation(true, || {
        let h = harness(Os::Windows);
        let w = windows_machine();
        h.mock.on_any_args("reg", CmdOutput::ok(""));
        *h.runner.silent_export.lock().unwrap() = true;
        let all = ids(&windows_scan(&h, &w));
        let e = with_registry(w.clone(), || {
            call(&h.ctx, "registry_cleaner.fix", json!({"issueIds": all})).unwrap_err()
        });
        assert!(e.message.contains("nothing was changed"));
        assert!(!calls(&h.mock).iter().any(|l| l.starts_with("reg delete")));
    });
}

#[test]
fn backup_false_and_bad_requests_are_rejected() {
    let h = harness(Os::Windows);
    let w = windows_machine();
    with_registry(w.clone(), || {
        for p in [
            json!({"issueIds": ["a"], "backup": false}),
            json!({"issueIds": []}),
            json!({"backup": true}),
            json!({"issueIds": ["a"], "categories": ["nope"]}),
        ] {
            assert_eq!(call(&h.ctx, "registry_cleaner.fix", p).unwrap_err().code, ErrorCode::InvalidParams);
        }
    });
    assert!(h.mock.calls().is_empty());
}

#[test]
fn per_issue_failures_are_reported_and_the_backup_stays() {
    with_elevation(true, || {
        let h = harness(Os::Windows);
        let w = windows_machine();
        h.mock.on("reg", &["delete", r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Run", "/v", "Gone", "/f"], CmdOutput::failed(1, "ERROR: Access is denied."));
        h.mock.on_any_args("reg", CmdOutput::ok(""));
        h.mock.on_any_args("powershell", CmdOutput::ok("R0:1:ERROR: Access is denied.\nR1:0:\n"));
        let all = ids(&windows_scan(&h, &w));
        let out = with_registry(w.clone(), || {
            call(&h.ctx, "registry_cleaner.fix", json!({"issueIds": all})).unwrap()
        });
        assert_eq!(out["fixed"], 2);
        assert_eq!(out["failed"], 2);
        let errors: Vec<&str> = out["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["ok"] == false)
            .map(|r| r["error"].as_str().unwrap())
            .collect();
        assert_eq!(errors.len(), 2);
        assert!(errors.iter().all(|e| e.contains("Access is denied")));
        assert_eq!(call(&h.ctx, "registry_cleaner.list_backups", json!({})).unwrap().as_array().unwrap().len(), 1);
    });
}

#[test]
fn windows_restore_imports_every_saved_key() {
    with_elevation(true, || {
        let h = harness(Os::Windows);
        let w = windows_machine();
        h.mock.on_any_args("reg", CmdOutput::ok(""));
        h.mock.on_any_args("powershell", CmdOutput::ok("R0:0:\nR1:0:\n"));
        let all = ids(&windows_scan(&h, &w));
        let out = with_registry(w.clone(), || {
            call(&h.ctx, "registry_cleaner.fix", json!({"issueIds": all})).unwrap()
        });
        let id = out["backupId"].as_str().unwrap().to_string();
        let before = h.mock.calls().len();
        let r = call(&h.ctx, "registry_cleaner.restore_backup", json!({"id": id})).unwrap();
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["restored"], 3);
        let after: Vec<String> = calls(&h.mock)[before..].to_vec();
        assert_eq!(after.iter().filter(|l| l.starts_with("reg import")).count(), 2, "{after:?}");
        assert_eq!(after.iter().filter(|l| l.starts_with("powershell")).count(), 1);
        assert!(after.iter().all(|l| !l.contains("delete")));
        // restoring on another platform is refused
        let mut lin = h.ctx.clone();
        lin.env.os = Os::Linux;
        assert_eq!(call(&lin, "registry_cleaner.restore_backup", json!({"id": id})).unwrap_err().code, ErrorCode::Unsupported);
    });
}

#[test]
fn restore_and_delete_validate_ids() {
    let h = harness(Os::Linux);
    for bad in ["", "..", "../x", "settings.json", "config-1/..", "registry-abc"] {
        for m in ["registry_cleaner.restore_backup", "registry_cleaner.delete_backup"] {
            let e = call(&h.ctx, m, json!({"id": bad})).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidParams, "{m} {bad}");
        }
    }
    for m in ["registry_cleaner.restore_backup", "registry_cleaner.delete_backup"] {
        assert_eq!(call(&h.ctx, m, json!({"id": "config-123"})).unwrap_err().code, ErrorCode::NotFound);
        assert_eq!(call(&h.ctx, m, json!({})).unwrap_err().code, ErrorCode::InvalidParams);
    }
}

#[test]
fn delete_backup_removes_only_that_backup() {
    with_elevation(true, || {
        let h = harness(Os::Windows);
        let w = windows_machine();
        h.mock.on_any_args("reg", CmdOutput::ok(""));
        h.mock.on_any_args("powershell", CmdOutput::ok("R0:0:\nR1:0:\n"));
        let all = ids(&windows_scan(&h, &w));
        let a = with_registry(w.clone(), || call(&h.ctx, "registry_cleaner.fix", json!({"issueIds": all.clone()})).unwrap());
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = with_registry(w.clone(), || call(&h.ctx, "registry_cleaner.fix", json!({"issueIds": all})).unwrap());
        let (ia, ib) = (a["backupId"].as_str().unwrap(), b["backupId"].as_str().unwrap());
        assert_ne!(ia, ib);
        let d = call(&h.ctx, "registry_cleaner.delete_backup", json!({"id": ia})).unwrap();
        assert!(d["freedBytes"].as_u64().unwrap() > 0);
        let list = call(&h.ctx, "registry_cleaner.list_backups", json!({})).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["id"], ib);
    });
}

// ---------------------------------------------------------------- Linux

#[cfg(unix)]
mod linux {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o7777
    }

    fn set_mode(p: &Path, m: u32) {
        fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
    }

    fn broken(exec: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName=Gone\nExec={exec}\n")
    }

    const MIME: &str = "# comments stay\n[Default Applications]\ntext/plain=gedit.desktop;\nimage/png=gone.desktop;eog.desktop;\n\n[Added Associations]\ntext/html=gone.desktop;\nx/y=gedit.desktop;gone.desktop;\n\n[Removed Associations]\ntext/x=gone.desktop;\n";

    /// A home with one of everything, plus valid neighbours that must survive.
    struct Fixture {
        h: H,
        launcher: PathBuf,
        autostart: PathBuf,
        link: PathBuf,
        service: PathBuf,
        wants: PathBuf,
        mimeapps: PathBuf,
        good_launcher: PathBuf,
        good_link: PathBuf,
    }

    fn fixture() -> Fixture {
        let h = harness(Os::Linux);
        let e = &h.ctx.env;
        write(&e.sys_path("/usr/bin/present"), "");
        write(&e.sys_path("/usr/share/applications/gedit.desktop"), "[Desktop Entry]\n");
        write(&e.sys_path("/usr/share/applications/eog.desktop"), "[Desktop Entry]\n");
        let launcher = e.user_data_dir.join("applications/oldapp.desktop");
        write(&launcher, &broken("/opt/gone/app %U"));
        set_mode(&launcher, 0o750);
        let good_launcher = e.user_data_dir.join("applications/good.desktop");
        write(&good_launcher, &broken("/usr/bin/present"));
        let autostart = e.config_dir.join("autostart/gone.desktop");
        write(&autostart, &broken("/opt/gone/auto"));
        let good_link = e.home.join(".local/bin/good");
        fs::create_dir_all(good_link.parent().unwrap()).unwrap();
        symlink("/usr/bin/present", &good_link).unwrap();
        let link = e.home.join(".local/bin/dead");
        symlink("/opt/gone/tool", &link).unwrap();
        let service = e.config_dir.join("systemd/user/gone.service");
        write(&service, "[Service]\nExecStart=/opt/gone/daemon\n");
        let wants = e.config_dir.join("systemd/user/default.target.wants/gone.service");
        fs::create_dir_all(wants.parent().unwrap()).unwrap();
        symlink("../gone.service", &wants).unwrap();
        let mimeapps = e.config_dir.join("mimeapps.list");
        write(&mimeapps, MIME);
        set_mode(&mimeapps, 0o640);
        Fixture {
            h,
            launcher,
            autostart,
            link,
            service,
            wants,
            mimeapps,
            good_launcher,
            good_link,
        }
    }

    fn scan(c: &Ctx) -> Value {
        call(c, "registry_cleaner.scan", json!({"categories": ["desktop_entries", "autostart", "broken_symlinks", "mime_associations", "user_services"]})).unwrap()
    }

    #[test]
    fn linux_scan_shape() {
        let f = fixture();
        let s = scan(&f.h.ctx);
        assert_eq!(s["platform"], "linux");
        assert_eq!(s["title"], "Config Issues");
        assert_eq!(s["counts"]["desktop_entries"], 1);
        assert_eq!(s["counts"]["autostart"], 1);
        assert_eq!(s["counts"]["broken_symlinks"], 1);
        assert_eq!(s["counts"]["mime_associations"], 3);
        assert_eq!(s["counts"]["user_services"], 1);
        assert_eq!(s["issues"].as_array().unwrap().len(), 7);
        // read-only
        assert_eq!(fs::read_to_string(&f.mimeapps).unwrap(), MIME);
        assert!(f.launcher.exists());
        let l = s["issues"].as_array().unwrap().iter().find(|i| i["category"] == "desktop_entries").unwrap();
        assert_eq!(l["location"], f.launcher.to_string_lossy().as_ref());
        assert_eq!(l["severity"], "low");
        assert_eq!(l["needsAdmin"], false);
    }

    #[test]
    fn fix_removes_edits_and_restore_puts_everything_back_exactly() {
        let f = fixture();
        let s = scan(&f.h.ctx);
        let before_mime = fs::read(&f.mimeapps).unwrap();
        let before_launcher = fs::read(&f.launcher).unwrap();
        let before_service = fs::read(&f.service).unwrap();
        let out = call(&f.h.ctx, "registry_cleaner.fix", json!({"issueIds": ids(&s), "backup": true, "categories": ["desktop_entries", "autostart", "broken_symlinks", "mime_associations", "user_services"]})).unwrap();
        assert_eq!(out["fixed"], 7, "{out}");
        assert_eq!(out["failed"], 0);
        let id = out["backupId"].as_str().unwrap().to_string();

        // the broken things are gone...
        for p in [&f.launcher, &f.autostart, &f.service] {
            assert!(!p.exists(), "{p:?}");
        }
        assert!(fs::symlink_metadata(&f.link).is_err());
        assert!(fs::symlink_metadata(&f.wants).is_err());
        // ...the good neighbours are untouched...
        assert!(f.good_launcher.exists());
        assert!(fs::symlink_metadata(&f.good_link).is_ok());
        // ...and mimeapps.list lost exactly the missing ids, nothing else
        assert_eq!(
            fs::read_to_string(&f.mimeapps).unwrap(),
            "# comments stay\n[Default Applications]\ntext/plain=gedit.desktop;\nimage/png=eog.desktop;\n\n[Added Associations]\nx/y=gedit.desktop;\n\n[Removed Associations]\ntext/x=gone.desktop;\n"
        );
        assert_eq!(mode(&f.mimeapps), 0o640);
        // a fresh scan is clean
        assert!(scan(&f.h.ctx)["issues"].as_array().unwrap().is_empty());

        // the backup lists what was fixed
        let list = call(&f.h.ctx, "registry_cleaner.list_backups", json!({})).unwrap();
        assert_eq!(list[0]["id"], id.as_str());
        assert_eq!(list[0]["issueCount"], 7);
        assert_eq!(list[0]["platform"], "linux");

        // restore: byte-identical content and mode bits, symlinks recreated
        let r = call(&f.h.ctx, "registry_cleaner.restore_backup", json!({"id": id})).unwrap();
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(fs::read(&f.mimeapps).unwrap(), before_mime);
        assert_eq!(mode(&f.mimeapps), 0o640);
        assert_eq!(fs::read(&f.launcher).unwrap(), before_launcher);
        assert_eq!(mode(&f.launcher), 0o750);
        assert_eq!(fs::read(&f.service).unwrap(), before_service);
        assert_eq!(fs::read_link(&f.link).unwrap(), Path::new("/opt/gone/tool"));
        assert_eq!(fs::read_link(&f.wants).unwrap(), Path::new("../gone.service"));
        assert!(f.autostart.exists());
        // the same problems are back
        assert_eq!(scan(&f.h.ctx)["issues"].as_array().unwrap().len(), 7);
    }

    #[test]
    fn selecting_a_subset_leaves_the_rest() {
        let f = fixture();
        let s = scan(&f.h.ctx);
        let only: Vec<String> = s["issues"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["category"] == "autostart")
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect();
        let out = call(&f.h.ctx, "registry_cleaner.fix", json!({"issueIds": only})).unwrap();
        assert_eq!(out["fixed"], 1);
        assert!(!f.autostart.exists());
        assert!(f.launcher.exists() && f.service.exists());
        assert_eq!(fs::read_to_string(&f.mimeapps).unwrap(), MIME);
    }

    #[test]
    fn one_mime_entry_only_touches_that_id() {
        let f = fixture();
        let s = scan(&f.h.ctx);
        let one = s["issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["category"] == "mime_associations" && i["value"] == "text/html")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let out = call(&f.h.ctx, "registry_cleaner.fix", json!({"issueIds": [one]})).unwrap();
        assert_eq!(out["fixed"], 1);
        assert_eq!(fs::read_to_string(&f.mimeapps).unwrap(), MIME.replace("text/html=gone.desktop;\n", ""));
    }

    #[test]
    fn if_the_backup_cannot_be_made_nothing_changes() {
        let f = fixture();
        let s = scan(&f.h.ctx);
        // <data>/backups is a plain file: no backup folder can be created
        fs::create_dir_all(&f.h.ctx.env.data_dir).unwrap();
        fs::write(f.h.ctx.env.data_dir.join("backups"), b"in the way").unwrap();
        let e = call(&f.h.ctx, "registry_cleaner.fix", json!({"issueIds": ids(&s)})).unwrap_err();
        assert!(matches!(e.code, ErrorCode::Io | ErrorCode::PermissionDenied | ErrorCode::NotFound), "{e:?}");
        assert!(f.launcher.exists() && f.autostart.exists() && f.service.exists());
        assert!(fs::symlink_metadata(&f.link).is_ok());
        assert_eq!(fs::read_to_string(&f.mimeapps).unwrap(), MIME);
    }

    #[test]
    fn an_item_that_changed_since_the_scan_is_refused() {
        let f = fixture();
        let s = scan(&f.h.ctx);
        let want = ids(&s);
        // A launcher that turned into a directory since the user's scan is not in the fresh
        // scan any more: refused for that id only.
        fs::remove_file(&f.launcher).unwrap();
        fs::create_dir(&f.launcher).unwrap();
        let out = call(&f.h.ctx, "registry_cleaner.fix", json!({"issueIds": want})).unwrap();
        assert!(out["results"].as_array().unwrap().iter().any(|r| r["ok"] == false));
        assert!(f.launcher.is_dir());
    }

    #[test]
    fn excluded_paths_are_not_deleted() {
        let f = fixture();
        crate::features::settings::update(&f.h.ctx, |s| {
            s.exclude.push(crate::features::settings::ExcludeEntry {
                id: "x".into(),
                pattern: f.launcher.to_string_lossy().into_owned(),
            });
        })
        .unwrap();
        let s = scan(&f.h.ctx);
        let out = call(&f.h.ctx, "registry_cleaner.fix", json!({"issueIds": ids(&s)})).unwrap();
        assert!(f.launcher.exists(), "excluded file must stay");
        let bad: Vec<_> = out["results"].as_array().unwrap().iter().filter(|r| r["ok"] == false).collect();
        assert_eq!(bad.len(), 1);
        assert!(bad[0]["error"].as_str().unwrap().contains("exclusion"));
    }

    #[test]
    fn system_launchers_are_removed_with_administrator_rights_and_restored() {
        with_elevation(false, || {
            let f = fixture();
            let e = &f.h.ctx.env;
            let sys = e.sys_path("/usr/share/applications/broken-sys.desktop");
            write(&sys, &broken("/opt/gone/sys"));
            set_mode(&sys, 0o644);
            f.h.mock.with_program("pkexec");
            let s = call(&f.h.ctx, "registry_cleaner.scan", json!({"categories": ["desktop_entries"]})).unwrap();
            let sysissue = s["issues"].as_array().unwrap().iter().find(|i| i["location"] == sys.to_string_lossy().as_ref()).unwrap();
            assert_eq!(sysissue["needsAdmin"], true);
            assert_eq!(sysissue["severity"], "medium");
            let original = fs::read(&sys).unwrap();
            let out = call(&f.h.ctx, "registry_cleaner.fix", json!({"issueIds": [sysissue["id"]], "categories": ["desktop_entries"]})).unwrap();
            assert_eq!(out["fixed"], 1, "{out}");
            assert!(!sys.exists());
            let c = calls(&f.h.mock);
            assert_eq!(c.iter().filter(|l| l.starts_with("pkexec rm -f --")).count(), 1, "{c:?}");
            assert!(c.iter().any(|l| l.ends_with("broken-sys.desktop")));
            // the backup copy was made before the privileged removal
            let r = call(&f.h.ctx, "registry_cleaner.restore_backup", json!({"id": out["backupId"]})).unwrap();
            assert_eq!(r["ok"], true, "{r}");
            assert_eq!(fs::read(&sys).unwrap(), original);
            assert_eq!(mode(&sys), 0o644);
            assert!(calls(&f.h.mock).iter().any(|l| l.starts_with("pkexec install -m 644")));
        });
    }

    #[test]
    fn a_denied_privileged_removal_is_reported_per_item() {
        with_elevation(false, || {
            let f = fixture();
            let sys = f.h.ctx.env.sys_path("/usr/share/applications/broken-sys.desktop");
            write(&sys, &broken("/opt/gone/sys"));
            // no pkexec at all
            let s = call(&f.h.ctx, "registry_cleaner.scan", json!({"categories": ["desktop_entries"]})).unwrap();
            let id = s["issues"].as_array().unwrap().iter().find(|i| i["location"] == sys.to_string_lossy().as_ref()).unwrap()["id"].clone();
            let out = call(&f.h.ctx, "registry_cleaner.fix", json!({"issueIds": [id], "categories": ["desktop_entries"]})).unwrap();
            assert_eq!(out["failed"], 1);
            assert!(sys.exists());
            assert!(out["results"][0]["error"].as_str().unwrap().contains("Administrator"));
        });
    }

    #[test]
    fn orphaned_packages_are_removed_by_name_after_a_dry_run() {
        with_elevation(true, || {
            let f = fixture();
            f.h.mock.on("apt-get", &["-s", "autoremove"], CmdOutput::ok("Remv liba [1]\nRemv libb [2]\n"));
            let s = call(&f.h.ctx, "registry_cleaner.scan", json!({"categories": ["orphaned_packages"]})).unwrap();
            assert_eq!(s["issues"].as_array().unwrap().len(), 2);
            let liba = s["issues"].as_array().unwrap().iter().find(|i| i["value"] == "liba").unwrap()["id"].clone();
            f.h.mock.on("apt-get", &["-s", "remove", "--", "liba"], CmdOutput::ok("Remv liba [1]\n"));
            f.h.mock.on("apt-get", &["remove", "-y", "--", "liba"], CmdOutput::ok(""));
            let out = call(&f.h.ctx, "registry_cleaner.fix", json!({"issueIds": [liba], "categories": ["orphaned_packages"]})).unwrap();
            assert_eq!(out["fixed"], 1, "{out}");
            let c = calls(&f.h.mock);
            let sim = c.iter().position(|l| l == "apt-get -s remove -- liba").unwrap();
            let real = c.iter().position(|l| l == "apt-get remove -y -- liba").unwrap();
            assert!(sim < real);
            // only the selected package was named
            assert!(!c.iter().any(|l| l.contains("libb") && l.contains("remove")));
            // and the backup remembers it, so restore can reinstall it
            f.h.mock.on("apt-get", &["install", "-y", "--", "liba"], CmdOutput::ok(""));
            let r = call(&f.h.ctx, "registry_cleaner.restore_backup", json!({"id": out["backupId"]})).unwrap();
            assert_eq!(r["ok"], true, "{r}");
            assert!(calls(&f.h.mock).contains(&"apt-get install -y -- liba".to_string()));
        });
    }

    #[test]
    fn a_removal_that_would_drag_other_packages_along_is_refused() {
        with_elevation(true, || {
            let f = fixture();
            f.h.mock.on("apt-get", &["-s", "autoremove"], CmdOutput::ok("Remv liba [1]\n"));
            let s = call(&f.h.ctx, "registry_cleaner.scan", json!({"categories": ["orphaned_packages"]})).unwrap();
            f.h.mock.on("apt-get", &["-s", "remove", "--", "liba"], CmdOutput::ok("Remv liba [1]\nRemv important [3]\n"));
            let out = call(&f.h.ctx, "registry_cleaner.fix", json!({"issueIds": ids(&s), "categories": ["orphaned_packages"]})).unwrap();
            assert_eq!(out["failed"], 1);
            assert!(out["results"][0]["error"].as_str().unwrap().contains("important"));
            assert!(!calls(&f.h.mock).iter().any(|l| l.starts_with("apt-get remove")));
        });
    }

    #[test]
    fn macos_launch_agents_and_links_round_trip() {
        let h = harness(Os::MacOs);
        let e = h.ctx.env.clone();
        let agent = e.home.join("Library/LaunchAgents/com.gone.plist");
        write(&agent, "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>Label</key><string>com.gone</string><key>Program</key><string>/opt/gone/x</string></dict></plist>");
        let lb = e.sys_path("/usr/local/bin");
        fs::create_dir_all(&lb).unwrap();
        symlink("../Cellar/gone/bin/gone", lb.join("gone")).unwrap();
        with_elevation(true, || {
            let s = call(&h.ctx, "registry_cleaner.scan", json!({})).unwrap();
            assert_eq!(s["platform"], "macos");
            assert_eq!(s["issues"].as_array().unwrap().len(), 2);
            let before = fs::read(&agent).unwrap();
            let out = call(&h.ctx, "registry_cleaner.fix", json!({"issueIds": ids(&s)})).unwrap();
            assert_eq!(out["fixed"], 2, "{out}");
            assert!(!agent.exists());
            assert!(fs::symlink_metadata(lb.join("gone")).is_err());
            let r = call(&h.ctx, "registry_cleaner.restore_backup", json!({"id": out["backupId"]})).unwrap();
            assert_eq!(r["ok"], true, "{r}");
            assert_eq!(fs::read(&agent).unwrap(), before);
            assert_eq!(fs::read_link(lb.join("gone")).unwrap(), Path::new("../Cellar/gone/bin/gone"));
        });
    }

    #[test]
    fn delete_backup_through_the_api() {
        let f = fixture();
        let s = scan(&f.h.ctx);
        let out = call(&f.h.ctx, "registry_cleaner.fix", json!({"issueIds": ids(&s)})).unwrap();
        let id = out["backupId"].as_str().unwrap();
        let dir = f.h.ctx.env.data_dir.join("backups").join(id);
        assert!(dir.is_dir());
        // settings written next to the backups survive
        crate::features::settings::update(&f.h.ctx, |s| s.language = "en".into()).unwrap();
        call(&f.h.ctx, "registry_cleaner.delete_backup", json!({"id": id})).unwrap();
        assert!(!dir.exists());
        assert!(f.h.ctx.env.data_dir.join("settings.json").exists());
        assert!(call(&f.h.ctx, "registry_cleaner.list_backups", json!({})).unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn errors_have_a_useful_shape() {
        let e: ApiError = call(&harness(Os::Linux).ctx, "registry_cleaner.fix", json!({"issueIds": ["x"], "backup": false})).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams);
        assert!(e.message.contains("mandatory"));
    }
}
