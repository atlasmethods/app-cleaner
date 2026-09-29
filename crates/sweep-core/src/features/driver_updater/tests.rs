//! Handler-level tests for the driver updater.

use serde_json::{json, Value};
use std::fs;

use super::*;
use crate::api::dispatch;
use crate::elevate::{powershell_decode, with_elevation};
use crate::runner::MockRunner;

fn ctx_for(os: Os) -> (tempfile::TempDir, Ctx, MockRunner) {
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
        .map(|(p, a)| std::iter::once(p).chain(a).collect::<Vec<_>>().join(" "))
        .collect()
}

const DEV: &str = "3fc0a2fa8d0ff8ec5b4d0d4dbbc91ba6a3d1d1d1";

const FWUPD_JSON: &str = r#"{"Devices":[{"Name":"System Firmware","DeviceId":"3fc0a2fa8d0ff8ec5b4d0d4dbbc91ba6a3d1d1d1","Flags":["updatable","needs-reboot"],"Vendor":"Dell Inc.","Version":"1.20.0","Releases":[{"Summary":"Firmware for XPS","Version":"1.22.0","Vendor":"Dell","Flags":["is-upgrade"]}]}]}"#;

const UBUNTU: &str = "== /sys/devices/pci0000:00/0000:01:00.0 ==\nvendor   : NVIDIA Corporation\nmodel    : GA106 [GeForce RTX 3060]\ndriver   : nvidia-driver-535 - distro non-free recommended\ndriver   : xserver-xorg-video-nouveau - distro free builtin\n";

#[test]
fn all_methods_are_registered() {
    for m in METHODS {
        assert!(crate::api::registry().get(m).is_some(), "{m}");
    }
}

// ---------------------------------------------------------------- scan (Linux)

#[test]
fn scan_fwupd_success() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "fwupdmgr",
        &["get-updates", "--json"],
        CmdOutput::ok(FWUPD_JSON),
    );
    let v = call(&c, "driver_updater.scan", Value::Null).unwrap();
    assert_eq!(calls(&m), ["fwupdmgr get-updates --json"]);
    let e = &v[0];
    assert_eq!(e["id"], format!("fwupd:{DEV}"));
    assert_eq!(e["deviceName"], "System Firmware");
    assert_eq!(e["currentVersion"], "1.20.0");
    assert_eq!(e["newVersion"], "1.22.0");
    assert_eq!(e["vendor"], "Dell");
    assert_eq!(e["source"], "fwupd");
    assert_eq!(e["description"], "Firmware for XPS");
    assert_eq!(e["rebootRequired"], true);
}

#[test]
fn scan_fwupd_exit_2_means_nothing_to_do_not_an_error() {
    for stdout in ["", "No updatable devices\n", "No updates available\n"] {
        let (_d, c, m) = ctx_for(Os::Linux);
        m.on(
            "fwupdmgr",
            &["get-updates", "--json"],
            CmdOutput {
                status: 2,
                stdout: stdout.into(),
                stderr: String::new(),
            },
        );
        assert_eq!(
            call(&c, "driver_updater.scan", Value::Null).unwrap(),
            json!([]),
            "{stdout:?}"
        );
    }
}

#[test]
fn scan_fwupd_real_failure_is_an_error() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "fwupdmgr",
        &["get-updates", "--json"],
        CmdOutput::failed(1, "Failed to connect to daemon: Could not connect"),
    );
    let e = call(&c, "driver_updater.scan", Value::Null).unwrap_err();
    assert_eq!(e.code, ErrorCode::Io);
    assert!(
        e.message.contains("Failed to connect to daemon"),
        "{}",
        e.message
    );
    // garbage output with exit 0
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "fwupdmgr",
        &["get-updates", "--json"],
        CmdOutput::ok("<html>"),
    );
    assert_eq!(
        call(&c, "driver_updater.scan", Value::Null)
            .unwrap_err()
            .code,
        ErrorCode::Io
    );
}

#[test]
fn scan_ubuntu_drivers_lists_only_uninstalled_recommended_drivers() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on("ubuntu-drivers", &["devices"], CmdOutput::ok(UBUNTU));
    m.on(
        "dpkg-query",
        &["-W", "-f=${db:Status-Status}", "nvidia-driver-535"],
        CmdOutput::failed(
            1,
            "dpkg-query: no packages found matching nvidia-driver-535",
        ),
    );
    let v = call(&c, "driver_updater.scan", Value::Null).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["id"], "ubuntu-drivers:nvidia-driver-535");
    assert_eq!(v[0]["deviceName"], "GA106 [GeForce RTX 3060]");
    assert_eq!(v[0]["vendor"], "NVIDIA Corporation");
    assert_eq!(v[0]["source"], "ubuntu-drivers");
    assert_eq!(v[0]["rebootRequired"], true);

    // already installed -> nothing to do
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on("ubuntu-drivers", &["devices"], CmdOutput::ok(UBUNTU));
    m.on(
        "dpkg-query",
        &["-W", "-f=${db:Status-Status}", "nvidia-driver-535"],
        CmdOutput::ok("installed"),
    );
    assert_eq!(
        call(&c, "driver_updater.scan", Value::Null).unwrap(),
        json!([])
    );
}

#[test]
fn scan_combines_sources_and_one_failing_tool_does_not_hide_the_other() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "fwupdmgr",
        &["get-updates", "--json"],
        CmdOutput::ok(FWUPD_JSON),
    );
    m.on("ubuntu-drivers", &["devices"], CmdOutput::failed(1, "boom"));
    let v = call(&c, "driver_updater.scan", Value::Null).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
}

#[test]
fn scan_without_any_tool_is_unsupported() {
    let (_d, c, _m) = ctx_for(Os::Linux);
    assert_eq!(
        call(&c, "driver_updater.scan", Value::Null)
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
    let (_d, c, _m) = ctx_for(Os::Windows);
    assert_eq!(
        call(&c, "driver_updater.scan", Value::Null)
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
}

// ---------------------------------------------------------------- scan (Windows, macOS)

fn ps_script_of(call: &(String, Vec<String>)) -> String {
    let i = call.1.iter().position(|a| a == "-EncodedCommand").unwrap();
    powershell_decode(&call.1[i + 1]).unwrap()
}

const WIN_JSON: &str = r#"[{"Title":"Intel - Net - 22.1.0.4","DriverModel":"Intel(R) Wi-Fi 6 AX201","DriverVerDate":"2023-05-01","DriverProvider":"Intel","UpdateID":"A1B2C3D4-0000-1111-2222-333344445555","RebootRequired":false}]"#;

#[test]
fn scan_windows_runs_the_com_search_script() {
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on_any_args("powershell", CmdOutput::ok(WIN_JSON));
    let v = call(&c, "driver_updater.scan", Value::Null).unwrap();
    let cs = m.calls();
    assert_eq!(cs.len(), 1);
    assert_eq!(
        &cs[0].1[..4],
        [
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass"
        ]
    );
    let script = ps_script_of(&cs[0]);
    assert!(script.contains("Microsoft.Update.Session"));
    assert!(script.contains("IsInstalled=0 and Type='Driver'"));
    assert!(script.contains("ConvertTo-Json"));
    assert_eq!(
        v[0]["id"],
        "windows-update:a1b2c3d4-0000-1111-2222-333344445555"
    );
    assert_eq!(v[0]["deviceName"], "Intel(R) Wi-Fi 6 AX201");
    assert_eq!(v[0]["newVersion"], "22.1.0.4");
    assert_eq!(v[0]["source"], "windows-update");
    // empty output = no drivers
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on_any_args("powershell", CmdOutput::ok(""));
    assert_eq!(
        call(&c, "driver_updater.scan", Value::Null).unwrap(),
        json!([])
    );
    // PowerShell error
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on_any_args(
        "powershell",
        CmdOutput::failed(1, "Exception calling \"Search\": 0x80072EE2"),
    );
    let e = call(&c, "driver_updater.scan", Value::Null).unwrap_err();
    assert_eq!(e.code, ErrorCode::Io);
    assert!(e.message.contains("0x80072EE2"));
}

#[test]
fn scan_macos_only_reports_firmware_items() {
    let (_d, c, m) = ctx_for(Os::MacOs);
    m.on(
        "softwareupdate",
        &["-l"],
        CmdOutput::ok("* Label: Safari17.5-17.5\n\tTitle: Safari, Version: 17.5, Size: 1KiB, Recommended: YES, \n* Label: MacBookAirEFIUpdate-1.0\n\tTitle: MacBook Air Firmware Update, Version: 1.0, Size: 1KiB, Recommended: YES, Action: restart, \n"),
    );
    let v = call(&c, "driver_updater.scan", Value::Null).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["id"], "macos:MacBookAirEFIUpdate-1.0");
    assert_eq!(v[0]["source"], "macos");
    assert_eq!(v[0]["rebootRequired"], true);
    // no firmware updates -> empty list
    let (_d, c, m) = ctx_for(Os::MacOs);
    m.on(
        "softwareupdate",
        &["-l"],
        CmdOutput::ok(
            "Software Update Tool\n\nFinding available software\nNo new software available.\n",
        ),
    );
    assert_eq!(
        call(&c, "driver_updater.scan", Value::Null).unwrap(),
        json!([])
    );
}

// ---------------------------------------------------------------- backup

#[test]
fn backup_is_windows_only_and_runs_pnputil_elevated() {
    for os in [Os::Linux, Os::MacOs] {
        let (_d, c, m) = ctx_for(os);
        let e = call(&c, "driver_updater.backup", Value::Null).unwrap_err();
        assert_eq!(e.code, ErrorCode::Unsupported);
        assert!(e.message.contains("only available on Windows"));
        assert!(m.calls().is_empty());
    }
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on_any_args(
        "pnputil",
        CmdOutput::ok("Total attempted: 42\nTotal exported: 42\n"),
    );
    let r = with_elevation(true, || call(&c, "driver_updater.backup", Value::Null)).unwrap();
    assert_eq!(r["ok"], true);
    let path = r["path"].as_str().unwrap().to_string();
    assert!(
        path.contains("backups") && path.contains("drivers-"),
        "{path}"
    );
    assert!(std::path::Path::new(&path).is_dir());
    assert_eq!(calls(&m), [format!("pnputil /export-driver * {path}")]);

    // not elevated: goes through the runas wrapper
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on_any_args("powershell", CmdOutput::ok("Total exported: 3\n"));
    let r = with_elevation(false, || call(&c, "driver_updater.backup", Value::Null)).unwrap();
    assert_eq!(r["ok"], true);
    let script = ps_script_of(&m.calls()[0]);
    assert!(
        script.contains(r#""pnputil" "/export-driver" "*""#),
        "{script}"
    );
    assert!(script.contains("-Verb RunAs"));
}

// ---------------------------------------------------------------- update

#[test]
fn update_fwupd_command_line_elevation_and_reboot_flag() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "fwupdmgr",
        &["get-updates", "--json"],
        CmdOutput::ok(FWUPD_JSON),
    );
    m.on(
        "pkexec",
        &["fwupdmgr", "update", DEV, "-y", "--no-reboot-check"],
        CmdOutput::ok("Successfully installed firmware\n"),
    );
    let r = with_elevation(false, || {
        call(
            &c,
            "driver_updater.update",
            json!({"ids": [format!("fwupd:{DEV}")]}),
        )
    })
    .unwrap();
    assert_eq!(
        calls(&m).last().unwrap(),
        &format!("pkexec fwupdmgr update {DEV} -y --no-reboot-check")
    );
    assert_eq!(r["succeeded"], 1);
    assert_eq!(r["rebootRequired"], true);
    assert_eq!(r["results"][0]["rebootRequired"], true);
    assert_eq!(r["results"][0]["ok"], true);

    // elevated + failure
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "fwupdmgr",
        &["get-updates", "--json"],
        CmdOutput::ok(FWUPD_JSON),
    );
    m.on(
        "fwupdmgr",
        &["update", DEV, "-y", "--no-reboot-check"],
        CmdOutput::failed(1, "Battery level is too low"),
    );
    let r = with_elevation(true, || {
        call(
            &c,
            "driver_updater.update",
            json!({"ids": [format!("fwupd:{DEV}")]}),
        )
    })
    .unwrap();
    assert_eq!(r["failed"], 1);
    assert_eq!(r["rebootRequired"], false);
    assert!(r["results"][0]["message"]
        .as_str()
        .unwrap()
        .contains("Battery level is too low"));
}

#[test]
fn update_ubuntu_drivers_command_line() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on("ubuntu-drivers", &["devices"], CmdOutput::ok(UBUNTU));
    m.on(
        "dpkg-query",
        &["-W", "-f=${db:Status-Status}", "nvidia-driver-535"],
        CmdOutput::failed(1, ""),
    );
    m.on(
        "pkexec",
        &["ubuntu-drivers", "install", "nvidia-driver-535"],
        CmdOutput::ok("done"),
    );
    let r = with_elevation(false, || {
        call(
            &c,
            "driver_updater.update",
            json!({"ids": ["ubuntu-drivers:nvidia-driver-535"]}),
        )
    })
    .unwrap();
    assert_eq!(
        calls(&m).last().unwrap(),
        "pkexec ubuntu-drivers install nvidia-driver-535"
    );
    assert_eq!(r["succeeded"], 1);
    assert_eq!(r["rebootRequired"], true);
}

#[test]
fn update_refuses_ids_that_the_scan_did_not_return() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "fwupdmgr",
        &["get-updates", "--json"],
        CmdOutput::ok(FWUPD_JSON),
    );
    m.on(
        "fwupdmgr",
        &["update", DEV, "-y", "--no-reboot-check"],
        CmdOutput::ok(""),
    );
    let r = with_elevation(true, || {
        call(
            &c,
            "driver_updater.update",
            json!({"ids": [format!("fwupd:{DEV}"), "fwupd:-rf", "ubuntu-drivers:evil; id", "windows-update:a1b2c3d4-0000-1111-2222-333344445555"]}),
        )
    })
    .unwrap();
    assert_eq!(r["succeeded"], 1);
    assert_eq!(r["failed"], 3);
    let executed: Vec<String> = calls(&m)
        .into_iter()
        .filter(|c| c.contains("update "))
        .collect();
    assert_eq!(executed.len(), 1);
    assert_eq!(
        call(&c, "driver_updater.update", json!({"ids": []}))
            .unwrap_err()
            .code,
        ErrorCode::InvalidParams
    );
}

#[test]
fn update_cancelled_authorization_aborts_the_rest() {
    let (_d, c, m) = ctx_for(Os::Linux);
    let two = r#"{"Devices":[{"Name":"A","DeviceId":"aa11","Releases":[{"Version":"2"}]},{"Name":"B","DeviceId":"bb22","Releases":[{"Version":"2"}]}]}"#;
    m.on("fwupdmgr", &["get-updates", "--json"], CmdOutput::ok(two));
    m.on(
        "pkexec",
        &["fwupdmgr", "update", "aa11", "-y", "--no-reboot-check"],
        CmdOutput::failed(126, ""),
    );
    let r = with_elevation(false, || {
        call(
            &c,
            "driver_updater.update",
            json!({"ids": ["fwupd:aa11", "fwupd:bb22"]}),
        )
    })
    .unwrap();
    assert_eq!(r["failed"], 2);
    assert_eq!(r["results"][0]["message"], "Authorization was cancelled");
    assert!(r["results"][1]["message"]
        .as_str()
        .unwrap()
        .starts_with("Not run"));
    assert!(!calls(&m).iter().any(|c| c.contains("bb22 -y")));
}

#[test]
fn install_windows_script_embeds_only_validated_ids() {
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on_any_args("powershell", CmdOutput::ok(WIN_JSON));
    let entries = collect(&c, &Job::detached()).unwrap();
    assert_eq!(entries.len(), 1);
    let install = r#"{"ResultCode":2,"RebootRequired":true,"Updates":[{"UpdateID":"A1B2C3D4-0000-1111-2222-333344445555","ResultCode":2,"HResult":0,"RebootRequired":true}]}"#;
    let (_d2, c2, m2) = ctx_for(Os::Windows);
    m2.on_any_args("powershell", CmdOutput::ok(install));
    let results =
        with_elevation(true, || install_windows(&c2, &Job::detached(), &entries)).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].ok && results[0].reboot_required);
    let script = ps_script_of(&m2.calls()[0]);
    assert!(
        script.contains("$ids = @('a1b2c3d4-0000-1111-2222-333344445555')"),
        "{script}"
    );
    assert!(script.contains("CreateUpdateInstaller"));
}

/// Runner whose `powershell` answers differ per call (scan first, then install).
struct Seq {
    powershell: std::sync::Mutex<std::collections::VecDeque<CmdOutput>>,
    log: std::sync::Mutex<Vec<String>>,
    pnputil: CmdOutput,
}

impl crate::runner::CommandRunner for Seq {
    fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput> {
        self.log
            .lock()
            .unwrap()
            .push(format!("{program} {}", args.first().unwrap_or(&"")));
        match program {
            "powershell" => Ok(self
                .powershell
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| CmdOutput::failed(1, "no more scripted output"))),
            "pnputil" => Ok(self.pnputil.clone()),
            _ => Err(ApiError::not_found(program)),
        }
    }
    fn which(&self, p: &str) -> Option<std::path::PathBuf> {
        matches!(p, "powershell" | "pnputil").then(|| std::path::PathBuf::from(p))
    }
}

fn win_ctx(seq: Seq) -> (tempfile::TempDir, Ctx, std::sync::Arc<Seq>) {
    let d = tempfile::tempdir().unwrap();
    let seq = std::sync::Arc::new(seq);
    let mut c = Ctx::test(d.path(), MockRunner::new());
    c.env.os = Os::Windows;
    c.runner = seq.clone();
    (d, c, seq)
}

#[test]
fn update_windows_full_flow_backup_then_install_with_per_driver_results() {
    let install = r#"{"ResultCode":3,"RebootRequired":true,"Updates":[{"UpdateID":"A1B2C3D4-0000-1111-2222-333344445555","ResultCode":2,"HResult":0,"RebootRequired":true}]}"#;
    let (_d, c, seq) = win_ctx(Seq {
        powershell: std::sync::Mutex::new([CmdOutput::ok(WIN_JSON), CmdOutput::ok(install)].into()),
        log: Default::default(),
        pnputil: CmdOutput::ok("Total exported: 5\n"),
    });
    let r = with_elevation(true, || {
        call(
            &c,
            "driver_updater.update",
            json!({"ids": ["windows-update:a1b2c3d4-0000-1111-2222-333344445555"]}),
        )
    })
    .unwrap();
    assert_eq!(r["succeeded"], 1, "{r}");
    assert_eq!(r["rebootRequired"], true);
    assert!(r["backupPath"].as_str().unwrap().contains("drivers-"));
    let log = seq.log.lock().unwrap().clone();
    assert_eq!(
        log,
        [
            "powershell -NoProfile",
            "pnputil /export-driver",
            "powershell -NoProfile"
        ]
    );
}

#[test]
fn update_windows_aborts_when_the_backup_fails() {
    let (_d, c, seq) = win_ctx(Seq {
        powershell: std::sync::Mutex::new([CmdOutput::ok(WIN_JSON)].into()),
        log: Default::default(),
        pnputil: CmdOutput::failed(5, "Access is denied."),
    });
    let e = with_elevation(true, || {
        call(
            &c,
            "driver_updater.update",
            json!({"ids": ["windows-update:a1b2c3d4-0000-1111-2222-333344445555"]}),
        )
    })
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Io);
    assert!(e.message.contains("nothing was updated"), "{}", e.message);
    // only the scan and the backup ran; no install script
    assert_eq!(seq.log.lock().unwrap().len(), 2);
}

#[test]
fn update_windows_reports_failed_and_missing_results() {
    let install = r#"{"ResultCode":4,"RebootRequired":false,"Updates":[{"UpdateID":"a1b2c3d4-0000-1111-2222-333344445555","ResultCode":4,"HResult":-2145116147}]}"#;
    let (_d, c, _seq) = win_ctx(Seq {
        powershell: std::sync::Mutex::new([CmdOutput::ok(WIN_JSON), CmdOutput::ok(install)].into()),
        log: Default::default(),
        pnputil: CmdOutput::ok(""),
    });
    let r = with_elevation(true, || {
        call(
            &c,
            "driver_updater.update",
            json!({"ids": ["windows-update:a1b2c3d4-0000-1111-2222-333344445555"]}),
        )
    })
    .unwrap();
    assert_eq!(r["failed"], 1);
    assert_eq!(r["results"][0]["exitCode"], 4);
    assert!(
        r["results"][0]["message"]
            .as_str()
            .unwrap()
            .contains("HRESULT"),
        "{r}"
    );
    assert_eq!(r["rebootRequired"], false);
}

#[test]
fn update_macos_uses_softwareupdate_elevated() {
    let (_d, c, m) = ctx_for(Os::MacOs);
    m.on(
        "softwareupdate",
        &["-l"],
        CmdOutput::ok("* Label: MacBookAirEFIUpdate-1.0\n\tTitle: MacBook Air Firmware Update, Version: 1.0, Size: 1KiB, Recommended: YES, Action: restart, \n"),
    );
    m.on(
        "softwareupdate",
        &["-i", "MacBookAirEFIUpdate-1.0"],
        CmdOutput::ok("Done."),
    );
    let r = with_elevation(true, || {
        call(
            &c,
            "driver_updater.update",
            json!({"ids": ["macos:MacBookAirEFIUpdate-1.0"]}),
        )
    })
    .unwrap();
    assert_eq!(r["succeeded"], 1);
    assert_eq!(r["rebootRequired"], true);
}
