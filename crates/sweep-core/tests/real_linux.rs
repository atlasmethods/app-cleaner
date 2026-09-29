//! Real-system tests against dpkg / apt. They install a tiny dummy package, so they are
//! `#[ignore]`d by default and only run explicitly, as root, on a Debian-family system:
//!
//! ```text
//! cargo test -p sweep-core -- --ignored real_
//! ```
//!
//! (`scripts/check.sh` does this when it runs as root with dpkg present.)

#![cfg(target_os = "linux")]

use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use sweep_core::{dispatch, Ctx, Env, Job, SystemRunner};

const PKG: &str = "clearsweep-dummy-test";
const VERSION: &str = "1.2.3-clearsweep1";

fn have(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// The real machine, but ClearSweep's own data (settings) goes to a throwaway directory.
fn real_ctx(data: &Path) -> Ctx {
    let mut env = Env::detect();
    env.data_dir = data.to_path_buf();
    Ctx::new(env, Arc::new(SystemRunner))
}

fn call(c: &Ctx, method: &str, params: Value) -> Result<Value, sweep_core::ApiError> {
    dispatch(c, method, params, &Job::detached())
}

fn installed(pkg: &str) -> bool {
    Command::new("dpkg-query")
        .args(["-W", "-f=${db:Status-Status}", pkg])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "installed")
        .unwrap_or(false)
}

/// Removes the dummy package even when an assertion fails.
struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = Command::new("dpkg").args(["-P", PKG]).output();
    }
}

fn build_and_install_dummy(tmp: &Path) {
    let root = tmp.join("pkg");
    fs::create_dir_all(root.join("DEBIAN")).unwrap();
    fs::create_dir_all(root.join("usr/share/clearsweep-dummy-test")).unwrap();
    fs::write(
        root.join("DEBIAN/control"),
        format!(
            "Package: {PKG}\nVersion: {VERSION}\nSection: misc\nPriority: optional\nArchitecture: all\n\
             Maintainer: ClearSweep Tests <tests@example.invalid>\nInstalled-Size: 8\n\
             Description: dummy package used by the ClearSweep test-suite\n"
        ),
    )
    .unwrap();
    fs::write(
        root.join("usr/share/clearsweep-dummy-test/hello.txt"),
        "hello\n",
    )
    .unwrap();
    let deb = tmp.join("dummy.deb");
    let out = Command::new("dpkg-deb")
        .args(["--build", "--root-owner-group"])
        .arg(&root)
        .arg(&deb)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "dpkg-deb: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = Command::new("dpkg").arg("-i").arg(&deb).output().unwrap();
    assert!(
        out.status.success(),
        "dpkg -i: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn should_run() -> bool {
    if !is_root() || !have("dpkg") || !have("dpkg-deb") || !have("dpkg-query") {
        eprintln!("skipping: needs root and dpkg (this is a Debian-family, root-only test)");
        return false;
    }
    true
}

#[test]
#[ignore = "installs a dummy package; run with --ignored real_"]
fn real_uninstall_list_and_run_with_dpkg() {
    if !should_run() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let _cleanup = Cleanup;
    let _ = Command::new("dpkg").args(["-P", PKG]).output();
    build_and_install_dummy(tmp.path());
    assert!(installed(PKG));
    let payload = Path::new("/usr/share/clearsweep-dummy-test/hello.txt");
    assert!(payload.exists());

    let c = real_ctx(&tmp.path().join("data"));
    let list = call(&c, "uninstall.list", Value::Null).unwrap();
    let entry = list
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == format!("dpkg:{PKG}"))
        .unwrap_or_else(|| {
            panic!(
                "dummy package not in the list ({} entries)",
                list.as_array().unwrap().len()
            )
        });
    assert_eq!(entry["name"], PKG);
    assert_eq!(entry["version"], VERSION);
    assert_eq!(entry["publisher"], "ClearSweep Tests");
    assert_eq!(entry["source"], "dpkg");
    assert_eq!(entry["isSystem"], false);
    assert_eq!(entry["uninstallable"], true);
    assert_eq!(entry["sizeBytes"], 8 * 1024);
    assert!(
        entry["installDate"].is_string(),
        "install date from dpkg info: {entry}"
    );

    // Essential packages are refused without force (nothing is executed).
    let e = call(&c, "uninstall.run", json!({"id": "dpkg:dpkg"})).unwrap_err();
    assert_eq!(e.code, sweep_core::ErrorCode::PermissionDenied, "{e:?}");
    assert!(installed("dpkg"));

    // Unknown ids never reach apt-get.
    let e = call(
        &c,
        "uninstall.run",
        json!({"id": "dpkg:definitely-not-installed-xyz"}),
    )
    .unwrap_err();
    assert_eq!(e.code, sweep_core::ErrorCode::NotFound);

    let r = call(&c, "uninstall.run", json!({"id": format!("dpkg:{PKG}")})).unwrap();
    assert_eq!(r["ok"], true, "uninstall failed: {r}");
    assert_eq!(r["exitCode"], 0);
    assert!(!installed(PKG), "package still installed");
    assert!(!payload.exists(), "payload file still present");

    let list = call(&c, "uninstall.list", Value::Null).unwrap();
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["id"] != format!("dpkg:{PKG}")));
}

#[test]
#[ignore = "talks to the real apt; run with --ignored real_"]
fn real_software_updater_list_smoke() {
    if !have("dpkg") || !have("apt") {
        eprintln!("skipping: no apt on this system");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let c = real_ctx(tmp.path());
    let v =
        call(&c, "software_updater.list", Value::Null).expect("apt list --upgradable must parse");
    for e in v.as_array().unwrap() {
        let id = e["id"].as_str().unwrap();
        assert!(id.contains(':'), "{id}");
        assert!(!e["name"].as_str().unwrap().is_empty());
        assert!(!e["newVersion"].as_str().unwrap().is_empty(), "{e}");
        assert!(e["ignored"] == false);
        if e["source"] == "apt" {
            assert!(!e["currentVersion"].as_str().unwrap().is_empty(), "{e}");
            assert!(e["security"].is_boolean());
        }
    }
    eprintln!(
        "real apt: {} upgradable package(s)",
        v.as_array().unwrap().len()
    );
}

#[test]
#[ignore = "runs real dpkg-query; run with --ignored real_"]
fn real_uninstall_list_marks_essential_packages_as_system() {
    if !have("dpkg-query") {
        eprintln!("skipping: no dpkg");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let c = real_ctx(tmp.path());
    let list = call(&c, "uninstall.list", Value::Null).unwrap();
    let a = list.as_array().unwrap();
    assert!(!a.is_empty());
    let dpkg = a
        .iter()
        .find(|e| e["id"] == "dpkg:dpkg")
        .expect("dpkg itself is installed");
    assert_eq!(dpkg["isSystem"], true);
    assert!(
        a.iter().any(|e| e["isSystem"] == false),
        "some non-system packages exist"
    );
    for e in a {
        assert!(e["id"].as_str().unwrap().contains(':'));
        assert!(!e["name"].as_str().unwrap().is_empty());
    }
}
