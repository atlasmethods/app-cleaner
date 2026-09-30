//! Run at startup: what is installed per OS, idempotence, and the `settings` integration.

use super::*;
use crate::api::dispatch;
use crate::fakesys::FakeSys;
use crate::job::Job;
use crate::testutil::Fixture;
use serde_json::{json, Value};
use std::sync::Arc;

const EXE: &str = "/usr/bin/clearsweep";

struct Bed {
    _tmp: tempfile::TempDir,
    fx: Fixture,
    ctx: Ctx,
    sys: FakeSys,
}

fn bed(os: Os, programs: &[&str]) -> Bed {
    let tmp = tempfile::tempdir().unwrap();
    let mut fx = Fixture::new(tmp.path());
    fx.env.os = os;
    let sys = FakeSys::new(programs);
    let ctx = Ctx::new(fx.env.clone(), Arc::new(sys.clone()));
    Bed {
        _tmp: tmp,
        fx,
        ctx,
        sys,
    }
}

fn exe() -> &'static Path {
    Path::new(EXE)
}

fn call(b: &Bed, m: &str, p: Value) -> Result<Value> {
    dispatch(&b.ctx, m, p, &Job::detached())
}

// ---------------------------------------------------------------- Linux

#[test]
fn linux_entry_is_a_valid_hidden_autostart_file() {
    let b = bed(Os::Linux, &[]);
    apply_with_exe(&b.ctx, true, exe()).unwrap();
    let path =
        b.fx.env
            .config_dir
            .join("autostart/clearsweep-agent.desktop");
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        "[Desktop Entry]\nType=Application\nName=ClearSweep background agent\nComment=Smart cleaning and scheduled maintenance for ClearSweep\nExec=/usr/bin/clearsweep agent\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n"
    );
    let d = xdg::parse_desktop(&text);
    assert_eq!(d.exec.as_deref(), Some("/usr/bin/clearsweep agent"));
    assert!(d.enabled());
    assert_eq!(is_installed(&b.ctx), Some(true));
    assert!(b.sys.calls().is_empty(), "no commands on Linux");
}

#[test]
fn linux_apply_is_idempotent_in_both_directions() {
    let b = bed(Os::Linux, &[]);
    let path = linux_path(&b.ctx);
    for _ in 0..3 {
        apply_with_exe(&b.ctx, true, exe()).unwrap();
        assert!(path.is_file());
    }
    for _ in 0..3 {
        apply_with_exe(&b.ctx, false, exe()).unwrap();
        assert!(!path.exists());
    }
    assert_eq!(is_installed(&b.ctx), Some(false));
    // Nothing else in the autostart folder is touched.
    let other = b.fx.env.config_dir.join("autostart/slack.desktop");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::write(&other, "[Desktop Entry]\nExec=slack\n").unwrap();
    apply_with_exe(&b.ctx, true, exe()).unwrap();
    apply_with_exe(&b.ctx, false, exe()).unwrap();
    assert!(other.exists());
}

#[test]
fn linux_entry_quotes_unusual_paths_per_the_desktop_entry_spec() {
    let b = bed(Os::Linux, &[]);
    apply_with_exe(
        &b.ctx,
        true,
        Path::new("/opt/My Apps/clear sweep/clearsweep"),
    )
    .unwrap();
    let text = fs::read_to_string(linux_path(&b.ctx)).unwrap();
    assert!(
        text.contains("Exec=\"/opt/My Apps/clear sweep/clearsweep\" agent\n"),
        "{text}"
    );
}

#[test]
fn a_disabled_entry_reads_as_not_installed_and_is_repaired_by_enabling() {
    let b = bed(Os::Linux, &[]);
    apply_with_exe(&b.ctx, true, exe()).unwrap();
    // The startup manager disabled it with Hidden=true.
    let path = linux_path(&b.ctx);
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, xdg::set_hidden(&text).unwrap()).unwrap();
    assert_eq!(is_installed(&b.ctx), Some(false));
    apply_with_exe(&b.ctx, true, exe()).unwrap();
    assert_eq!(is_installed(&b.ctx), Some(true));
}

#[test]
fn a_relative_executable_is_refused_and_nothing_is_written() {
    let b = bed(Os::Linux, &[]);
    let e = apply_with_exe(&b.ctx, true, Path::new("clearsweep")).unwrap_err();
    assert!(e.message.contains("not absolute"), "{}", e.message);
    assert!(!linux_path(&b.ctx).exists());
}

// ---------------------------------------------------------------- macOS

#[test]
fn macos_installs_a_launch_agent_and_bootstraps_it() {
    let b = bed(Os::MacOs, &["launchctl", "id"]);
    let path =
        b.fx.env
            .home
            .join("Library/LaunchAgents/app.clearsweep.agent.plist");
    apply_with_exe(&b.ctx, true, exe()).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let d = plist::Value::from_reader_xml(text.as_bytes())
        .unwrap()
        .into_dictionary()
        .unwrap();
    assert_eq!(
        d.get("Label").unwrap().as_string(),
        Some("app.clearsweep.agent")
    );
    assert_eq!(d.get("RunAtLoad").unwrap().as_boolean(), Some(true));
    assert_eq!(d.get("KeepAlive").unwrap().as_boolean(), Some(false));
    let args: Vec<&str> = d
        .get("ProgramArguments")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_string())
        .collect();
    assert_eq!(args, [EXE, "agent"]);
    assert_eq!(
        b.sys.calls(),
        [
            "id -u".to_string(),
            "launchctl bootout gui/501/app.clearsweep.agent".to_string(),
            format!("launchctl bootstrap gui/501 {}", path.display()),
        ]
    );
    assert_eq!(is_installed(&b.ctx), Some(true));

    b.sys.clear_calls();
    apply_with_exe(&b.ctx, false, exe()).unwrap();
    assert!(!path.exists());
    assert_eq!(
        b.sys.calls(),
        ["id -u", "launchctl bootout gui/501/app.clearsweep.agent"]
    );
    assert_eq!(is_installed(&b.ctx), Some(false));
    // Removing again is fine.
    apply_with_exe(&b.ctx, false, exe()).unwrap();
}

#[test]
fn macos_bootstrap_failure_is_an_error() {
    let b = bed(Os::MacOs, &["launchctl", "id"]);
    b.sys.fail("launchctl bootstrap", 5, "Bootstrap failed: 5");
    let e = apply_with_exe(&b.ctx, true, exe()).unwrap_err();
    assert!(e.message.contains("Bootstrap failed"), "{}", e.message);
}

// ---------------------------------------------------------------- Windows

const RUN: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

#[test]
fn windows_writes_and_removes_the_run_value() {
    let b = bed(Os::Windows, &["reg"]);
    apply_with_exe(&b.ctx, true, exe()).unwrap();
    assert_eq!(
        b.sys.calls(),
        [
            format!(r#"reg add {RUN} /v ClearSweepAgent /t REG_SZ /d "{EXE}" agent /f"#),
            format!("reg add {APPROVED} /v ClearSweepAgent /t REG_BINARY /d 020000000000000000000000 /f"),
        ]
    );

    // Present: query, delete the value and its approval record.
    b.sys.clear_calls();
    apply_with_exe(&b.ctx, false, exe()).unwrap();
    assert_eq!(
        b.sys.calls(),
        [
            format!("reg query {RUN} /v ClearSweepAgent"),
            format!("reg delete {RUN} /v ClearSweepAgent /f"),
            format!("reg delete {APPROVED} /v ClearSweepAgent /f"),
        ]
    );

    // Absent: only the query, no error.
    let b = bed(Os::Windows, &["reg"]);
    b.sys.fail(
        "reg query",
        1,
        "ERROR: The system was unable to find the specified registry key or value.",
    );
    apply_with_exe(&b.ctx, false, exe()).unwrap();
    assert_eq!(
        b.sys.calls(),
        [format!("reg query {RUN} /v ClearSweepAgent")]
    );
}

#[test]
fn windows_state_is_read_from_the_registry() {
    let b = bed(Os::Windows, &["reg"]);
    assert_eq!(is_installed(&b.ctx), Some(true));
    let b = bed(Os::Windows, &["reg"]);
    b.sys.fail("reg query", 1, "not found");
    assert_eq!(is_installed(&b.ctx), Some(false));
    // Anything else (access denied, reg missing) is "unknown", never "false".
    let b = bed(Os::Windows, &["reg"]);
    b.sys.fail("reg query", 2, "denied");
    assert_eq!(is_installed(&b.ctx), None);
    assert_eq!(is_installed(&bed(Os::Windows, &[]).ctx), None);
}

#[test]
fn windows_reg_failure_is_an_error() {
    let b = bed(Os::Windows, &["reg"]);
    b.sys.fail("reg add", 1, "ERROR: Access is denied.");
    let e = apply_with_exe(&b.ctx, true, exe()).unwrap_err();
    assert!(e.message.contains("Access is denied"), "{}", e.message);
}

// ---------------------------------------------------------------- settings integration

#[test]
fn settings_set_installs_and_removes_the_entry_and_get_reports_reality() {
    let b = bed(Os::Linux, &[]);
    let path = linux_path(&b.ctx);
    let v = call(&b, "settings.set", json!({"runAtStartup": true})).unwrap();
    assert_eq!(v["runAtStartup"], true);
    assert!(path.is_file());
    let text = fs::read_to_string(&path).unwrap();
    // The entry runs whatever executable is running now (the test binary here).
    assert!(text.contains(" agent\n"), "{text}");
    assert_eq!(
        call(&b, "settings.get", Value::Null).unwrap()["runAtStartup"],
        true
    );
    // Setting it again is harmless.
    call(&b, "settings.set", json!({"runAtStartup": true})).unwrap();

    // The entry removed behind our back: get reports the truth, and an unrelated set
    // does not write the stale value back.
    fs::remove_file(&path).unwrap();
    assert_eq!(
        call(&b, "settings.get", Value::Null).unwrap()["runAtStartup"],
        false
    );
    let v = call(&b, "settings.set", json!({"theme": "dark"})).unwrap();
    assert_eq!(v["runAtStartup"], false);
    let saved: Value =
        serde_json::from_slice(&fs::read(b.ctx.env.data_dir.join("settings.json")).unwrap())
            .unwrap();
    assert_eq!(saved["runAtStartup"], false);

    call(&b, "settings.set", json!({"runAtStartup": true})).unwrap();
    let v = call(&b, "settings.set", json!({"runAtStartup": false})).unwrap();
    assert_eq!(v["runAtStartup"], false);
    assert!(!path.exists());
}

#[test]
fn a_failed_install_is_reported_and_not_saved() {
    let b = bed(Os::Windows, &["reg"]);
    b.sys.fail("reg add", 1, "ERROR: Access is denied.");
    let e = call(
        &b,
        "settings.set",
        json!({"runAtStartup": true, "theme": "dark"}),
    )
    .unwrap_err();
    assert!(e.message.contains("Access is denied"), "{}", e.message);
    // Nothing of the patch was saved.
    let file = b.ctx.env.data_dir.join("settings.json");
    assert!(!file.exists(), "settings.json must not be written");
}

#[test]
fn reset_removes_the_entry() {
    let b = bed(Os::Linux, &[]);
    call(&b, "settings.set", json!({"runAtStartup": true})).unwrap();
    assert!(linux_path(&b.ctx).exists());
    let v = call(&b, "settings.reset", Value::Null).unwrap();
    assert_eq!(v["runAtStartup"], false);
    assert!(!linux_path(&b.ctx).exists());
}

#[test]
fn windows_settings_get_reconciles_through_reg_query() {
    let b = bed(Os::Windows, &["reg"]);
    b.sys.fail("reg query", 1, "not found");
    // Saved as true, but the Run value is gone.
    fs::create_dir_all(&b.ctx.env.data_dir).unwrap();
    fs::write(
        b.ctx.env.data_dir.join("settings.json"),
        r#"{"runAtStartup": true}"#,
    )
    .unwrap();
    assert_eq!(
        call(&b, "settings.get", Value::Null).unwrap()["runAtStartup"],
        false
    );
    // reg unavailable: the saved value is left alone.
    let b = bed(Os::Windows, &[]);
    fs::create_dir_all(&b.ctx.env.data_dir).unwrap();
    fs::write(
        b.ctx.env.data_dir.join("settings.json"),
        r#"{"runAtStartup": true}"#,
    )
    .unwrap();
    assert_eq!(
        call(&b, "settings.get", Value::Null).unwrap()["runAtStartup"],
        true
    );
}
