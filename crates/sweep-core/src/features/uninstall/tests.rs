//! Handler-level tests: exact command lines through `MockRunner`, elevation wrapping,
//! system-package refusal, leftover safety, and the Windows registry flows via a fake.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use super::linux::{DPKG_FORMAT, FLATPAK_COLUMNS, RPM_FORMAT};
use super::model::Hive;
use super::windows::{RegEntry, UninstallRegistry};
use super::*;
use crate::api::dispatch;
use crate::elevate::{powershell_decode, with_elevation};
use crate::error::{ErrorCode, Result};
use crate::runner::{CmdOutput, MockRunner};

fn ctx_for(os: Os) -> (tempfile::TempDir, Ctx, MockRunner) {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let mut c = Ctx::test(d.path(), m.clone());
    c.env.os = os;
    fs::create_dir_all(&c.env.home).unwrap();
    fs::create_dir_all(c.env.root.join("tmp")).unwrap();
    (d, c, m)
}

fn call(c: &Ctx, method: &str, params: Value) -> Result<Value> {
    dispatch(c, method, params, &Job::detached())
}

const DPKG: &str = "\
bash\t5.2.21-2ubuntu4\t1844\tUbuntu Developers <x@y.z>\trequired\tyes\tinstalled
firefox\t126.0\t257984\tUbuntu Mozilla Team <m@y.z>\toptional\t\tinstalled
gimp\t2.10.36\t28320\tUbuntu Developers <x@y.z>\toptional\t\tinstalled
";

fn script_dpkg(m: &MockRunner) {
    m.on("dpkg-query", &["-W", DPKG_FORMAT], CmdOutput::ok(DPKG));
}

fn script_apt_sim(m: &MockRunner, pkg: &str, out: &str) {
    m.on("apt-get", &["-s", "remove", pkg], CmdOutput::ok(out));
}

fn calls(m: &MockRunner) -> Vec<String> {
    m.calls()
        .into_iter()
        .map(|(p, a)| std::iter::once(p).chain(a).collect::<Vec<_>>().join(" "))
        .collect()
}

#[test]
fn all_methods_are_registered() {
    for m in METHODS {
        assert!(crate::api::registry().get(m).is_some(), "{m}");
    }
    assert_eq!(METHODS.len(), 7);
}

// ---------------------------------------------------------------- list

#[test]
fn list_queries_only_installed_managers_and_sorts() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_dpkg(&m);
    m.on(
        "flatpak",
        &["list", "--app", FLATPAK_COLUMNS],
        CmdOutput::ok("org.gimp.GIMP\tGIMP\t2.10.36\t1.2\u{a0}GB\tflathub\n"),
    );
    m.on(
        "snap",
        &["list"],
        CmdOutput::ok("Name Version Rev Tracking Publisher Notes\nsnapd 2.63 21759 latest/stable canonical** snapd\n"),
    );
    let v = call(&c, "uninstall.list", Value::Null).unwrap();
    let names: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["bash", "firefox", "gimp", "GIMP", "snapd"]);
    // exact command lines; rpm / pacman / brew were never touched
    assert_eq!(
        calls(&m),
        [
            format!("dpkg-query -W {DPKG_FORMAT}"),
            format!("flatpak list --app {FLATPAK_COLUMNS}"),
            "snap list".to_string(),
        ]
    );
    let ff = v
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "dpkg:firefox")
        .unwrap();
    assert_eq!(ff["source"], "dpkg");
    assert_eq!(ff["isSystem"], false);
    assert_eq!(ff["uninstallable"], true);
    assert_eq!(ff["canRepair"], false);
    assert_eq!(ff["canModify"], false);
    assert_eq!(ff["publisher"], "Ubuntu Mozilla Team");
    assert_eq!(ff["sizeBytes"], 257984 * 1024);
    let bash = v
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "dpkg:bash")
        .unwrap();
    assert_eq!(bash["isSystem"], true);
    // camelCase keys, optional ones omitted
    assert!(bash.get("installDate").is_none());
    assert!(bash.get("icon").is_none());
}

#[test]
fn list_uses_rpm_and_pacman_when_present() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "rpm",
        &["-qa", "--queryformat", RPM_FORMAT],
        CmdOutput::ok("firefox\t126.0-1.fc40\t1000\tFedora Project\t1716000000\n"),
    );
    m.on(
        "env",
        &["LC_ALL=C", "pacman", "-Qi"],
        CmdOutput::ok(
            "Name : htop\nVersion : 3.3.0-1\nInstalled Size : 400.00 KiB\nPackager : A <a@b.c>\n",
        ),
    );
    m.with_program("pacman");
    let v = call(&c, "uninstall.list", json!({})).unwrap();
    let ids: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["rpm:firefox", "pacman:htop"]);
}

#[test]
fn a_failing_tool_is_skipped_not_fatal() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "dpkg-query",
        &["-W", DPKG_FORMAT],
        CmdOutput::failed(2, "dpkg: error"),
    );
    m.on(
        "snap",
        &["list"],
        CmdOutput::ok("Name Version Rev Tracking Publisher Notes\nfoo 1 5 latest - -\n"),
    );
    let v = call(&c, "uninstall.list", Value::Null).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    // nothing installed and no tools at all -> just an empty list
    let (_d2, c2, _m2) = ctx_for(Os::Linux);
    assert_eq!(call(&c2, "uninstall.list", Value::Null).unwrap(), json!([]));
}

// ---------------------------------------------------------------- run: dpkg + elevation

#[test]
fn run_dpkg_elevated_runs_apt_get_directly_after_a_dry_run() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_dpkg(&m);
    script_apt_sim(&m, "firefox", "Remv firefox [126.0]\n");
    m.on(
        "apt-get",
        &["remove", "-y", "firefox"],
        CmdOutput::ok("Removing firefox ...\n"),
    );
    let r = with_elevation(true, || {
        call(&c, "uninstall.run", json!({"id": "dpkg:firefox"}))
    })
    .unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["exitCode"], 0);
    assert_eq!(r["needsForce"], false);
    assert_eq!(
        calls(&m),
        [
            format!("dpkg-query -W {DPKG_FORMAT}"),
            "apt-get -s remove firefox".to_string(),
            "apt-get remove -y firefox".to_string(),
        ]
    );
}

#[test]
fn run_dpkg_not_elevated_is_wrapped_in_pkexec() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_dpkg(&m);
    script_apt_sim(&m, "gimp", "Remv gimp [2.10.36]\n");
    m.on(
        "pkexec",
        &["apt-get", "remove", "-y", "gimp"],
        CmdOutput::ok("done"),
    );
    let r = with_elevation(false, || {
        call(&c, "uninstall.run", json!({"id": "dpkg:gimp"}))
    })
    .unwrap();
    assert_eq!(r["ok"], true);
    assert_eq!(calls(&m).last().unwrap(), "pkexec apt-get remove -y gimp");
}

#[test]
fn run_dpkg_declined_authorization_is_permission_denied() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_dpkg(&m);
    script_apt_sim(&m, "gimp", "Remv gimp [2.10.36]\n");
    m.on(
        "pkexec",
        &["apt-get", "remove", "-y", "gimp"],
        CmdOutput::failed(126, ""),
    );
    let e = with_elevation(false, || {
        call(&c, "uninstall.run", json!({"id": "dpkg:gimp"}))
    })
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert_eq!(e.message, "Authorization was cancelled");
}

#[test]
fn run_dpkg_failure_reports_exit_code_and_stderr_and_no_leftover_scan() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_dpkg(&m);
    script_apt_sim(&m, "gimp", "Remv gimp [2.10.36]\n");
    m.on(
        "apt-get",
        &["remove", "-y", "gimp"],
        CmdOutput::failed(100, "E: Could not get lock /var/lib/dpkg/lock-frontend"),
    );
    let r = with_elevation(true, || {
        call(&c, "uninstall.run", json!({"id": "dpkg:gimp"}))
    })
    .unwrap();
    assert_eq!(r["ok"], false);
    assert_eq!(r["exitCode"], 100);
    assert!(
        r["message"]
            .as_str()
            .unwrap()
            .contains("Could not get lock"),
        "{r}"
    );
    assert_eq!(r["leftovers"], json!([]));
}

#[test]
fn run_refuses_system_packages_without_force_and_allows_with_force() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_dpkg(&m);
    let e = with_elevation(true, || {
        call(&c, "uninstall.run", json!({"id": "dpkg:bash"}))
    })
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(e.message.contains("system component"), "{}", e.message);
    assert!(
        !calls(&m).iter().any(|c| c.contains("remove")),
        "{:?}",
        calls(&m)
    );

    m.on(
        "apt-get",
        &["remove", "-y", "bash"],
        CmdOutput::failed(100, "E: essential"),
    );
    let r = with_elevation(true, || {
        call(
            &c,
            "uninstall.run",
            json!({"id": "dpkg:bash", "force": true}),
        )
    })
    .unwrap();
    assert_eq!(r["exitCode"], 100);
    assert!(calls(&m).contains(&"apt-get remove -y bash".to_string()));
}

#[test]
fn run_reports_dependents_and_only_proceeds_when_forced() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_dpkg(&m);
    script_apt_sim(
        &m,
        "gimp",
        "Reading package lists...\nRemv gimp [2.10.36]\nRemv gimp-plugin-registry [1.0]\nRemv gimp:i386 [2.10.36]\n",
    );
    let r = with_elevation(true, || {
        call(&c, "uninstall.run", json!({"id": "dpkg:gimp"}))
    })
    .unwrap();
    assert_eq!(r["ok"], false);
    assert_eq!(r["needsForce"], true);
    assert_eq!(r["alsoRemoves"], json!(["gimp-plugin-registry"]));
    assert!(!calls(&m).contains(&"apt-get remove -y gimp".to_string()));

    m.on("apt-get", &["remove", "-y", "gimp"], CmdOutput::ok(""));
    let r = with_elevation(true, || {
        call(
            &c,
            "uninstall.run",
            json!({"id": "dpkg:gimp", "force": true}),
        )
    })
    .unwrap();
    assert_eq!(r["ok"], true);
}

#[test]
fn run_rejects_unknown_ids_and_never_builds_commands_from_them() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_dpkg(&m);
    for id in [
        "dpkg:nothing",
        "dpkg:firefox; rm -rf /",
        "dpkg:-y",
        "",
        "windows:hklm64:x",
    ] {
        let e = call(&c, "uninstall.run", json!({"id": id})).unwrap_err();
        assert!(
            matches!(e.code, ErrorCode::NotFound | ErrorCode::InvalidParams),
            "{id}: {e:?}"
        );
    }
    assert!(!calls(&m).iter().any(|c| c.contains("remove")));
    assert_eq!(
        call(&c, "uninstall.run", json!({})).unwrap_err().code,
        ErrorCode::InvalidParams
    );
}

// ---------------------------------------------------------------- run: other managers

#[test]
fn run_rpm_uses_dnf_after_checking_requirements() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "rpm",
        &["-qa", "--queryformat", RPM_FORMAT],
        CmdOutput::ok("firefox\t126.0-1.fc40\t1000\tFedora Project\t1716000000\nlibfoo\t1-1\t10\t(none)\t1716000000\n"),
    );
    m.on(
        "rpm",
        &["-q", "--whatrequires", "firefox"],
        CmdOutput::failed(1, "no package requires firefox\n"),
    );
    m.on(
        "dnf",
        &["remove", "-y", "firefox"],
        CmdOutput::ok("Complete!\n"),
    );
    let r = with_elevation(true, || {
        call(&c, "uninstall.run", json!({"id": "rpm:firefox"}))
    })
    .unwrap();
    assert_eq!(r["ok"], true);
    assert!(calls(&m).contains(&"dnf remove -y firefox".to_string()));
    // a library other packages need is reported instead of removed
    m.on(
        "rpm",
        &["-q", "--whatrequires", "libfoo"],
        CmdOutput::ok("bar-1.0-1.fc40.x86_64\nbaz-2.0-1.fc40.x86_64\n"),
    );
    let r = with_elevation(true, || {
        call(&c, "uninstall.run", json!({"id": "rpm:libfoo"}))
    })
    .unwrap();
    assert_eq!(r["needsForce"], true);
    assert_eq!(r["alsoRemoves"].as_array().unwrap().len(), 2);
    assert!(!calls(&m).contains(&"dnf remove -y libfoo".to_string()));
}

#[test]
fn run_pacman_flatpak_snap_and_brew_command_lines() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "env",
        &["LC_ALL=C", "pacman", "-Qi"],
        CmdOutput::ok("Name : htop\nVersion : 3.3.0-1\nInstalled Size : 400.00 KiB\n"),
    );
    m.with_program("pacman");
    m.on(
        "flatpak",
        &["list", "--app", FLATPAK_COLUMNS],
        CmdOutput::ok("org.gimp.GIMP\tGIMP\t2.10.36\t1\u{a0}GB\tflathub\n"),
    );
    m.on(
        "snap",
        &["list"],
        CmdOutput::ok(
            "Name Version Rev Tracking Publisher Notes\nvlc 3.0 100 latest/stable videolan** -\n",
        ),
    );
    m.on(
        "brew",
        &["list", "--versions"],
        CmdOutput::ok("wget 1.24.5\n"),
    );
    m.on(
        "brew",
        &["list", "--cask", "--versions"],
        CmdOutput::ok("firefox 126.0\n"),
    );
    m.on("pacman", &["-R", "--noconfirm", "htop"], CmdOutput::ok(""));
    m.on(
        "flatpak",
        &["uninstall", "-y", "org.gimp.GIMP"],
        CmdOutput::ok(""),
    );
    m.on("snap", &["remove", "vlc"], CmdOutput::ok("vlc removed"));
    m.on("brew", &["uninstall", "wget"], CmdOutput::ok(""));
    m.on(
        "brew",
        &["uninstall", "--cask", "firefox"],
        CmdOutput::ok(""),
    );
    for id in [
        "pacman:htop",
        "flatpak:org.gimp.GIMP",
        "snap:vlc",
        "brew:wget",
        "brew-cask:firefox",
    ] {
        let r = with_elevation(true, || call(&c, "uninstall.run", json!({"id": id}))).unwrap();
        assert_eq!(r["ok"], true, "{id}: {r}");
    }
    let cs = calls(&m);
    for expected in [
        "pacman -R --noconfirm htop",
        "flatpak uninstall -y org.gimp.GIMP",
        "snap remove vlc",
        "brew uninstall wget",
        "brew uninstall --cask firefox",
    ] {
        assert!(cs.contains(&expected.to_string()), "{expected} in {cs:?}");
    }
    // snap remove is elevated when we are not root; flatpak and brew never are
    let (_d2, c2, m2) = ctx_for(Os::Linux);
    m2.on(
        "snap",
        &["list"],
        CmdOutput::ok(
            "Name Version Rev Tracking Publisher Notes\nvlc 3.0 100 latest/stable videolan** -\n",
        ),
    );
    m2.on("pkexec", &["snap", "remove", "vlc"], CmdOutput::ok(""));
    let r = with_elevation(false, || {
        call(&c2, "uninstall.run", json!({"id": "snap:vlc"}))
    })
    .unwrap();
    assert_eq!(r["ok"], true);
    assert_eq!(calls(&m2).last().unwrap(), "pkexec snap remove vlc");
}

#[test]
fn run_appimage_deletes_file_and_launcher_via_safe_deleter() {
    let (_d, c, _m) = ctx_for(Os::Linux);
    let apps = c.env.home.join("Applications");
    fs::create_dir_all(&apps).unwrap();
    let img = apps.join("Obsidian-1.5.3.AppImage");
    fs::write(&img, "x").unwrap();
    let dd = c.env.user_data_dir.join("applications");
    fs::create_dir_all(&dd).unwrap();
    fs::write(
        dd.join("obsidian.desktop"),
        format!("[Desktop Entry]\nExec={} %U\n", img.display()),
    )
    .unwrap();
    let cfg = c.env.config_dir.join("Obsidian");
    fs::create_dir_all(&cfg).unwrap();
    fs::write(cfg.join("state.json"), "{}").unwrap();
    let id = format!("appimage:{}", img.display());
    let r = call(&c, "uninstall.run", json!({"id": id})).unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert!(!img.exists());
    assert!(!dd.join("obsidian.desktop").exists());
    // the config dir is only *reported* as a leftover, not deleted yet
    assert!(cfg.exists());
    let left = r["leftovers"].as_array().unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["path"], cfg.to_string_lossy().as_ref());
    assert_eq!(left[0]["kind"], "config");
    // gone from the list afterwards
    let l = call(&c, "uninstall.list", Value::Null).unwrap();
    assert!(l.as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------- leftovers

#[test]
fn remove_leftovers_deletes_only_what_the_scan_returns() {
    let (_d, c, _m) = ctx_for(Os::Linux);
    let mine = c.env.config_dir.join("Obsidian");
    fs::create_dir_all(mine.join("cache")).unwrap();
    fs::write(mine.join("cache/x"), "12345").unwrap();
    let other = c.env.config_dir.join("SomethingElse");
    fs::create_dir_all(&other).unwrap();
    let docs = c.env.home.join("Documents");
    fs::create_dir_all(&docs).unwrap();
    fs::write(docs.join("keep.txt"), "keep").unwrap();

    let scan = call(
        &c,
        "uninstall.leftovers",
        json!({"name": "Obsidian", "id": "appimage:/x/Obsidian.AppImage"}),
    )
    .unwrap();
    assert_eq!(scan.as_array().unwrap().len(), 1);

    let r = call(
        &c,
        "uninstall.remove_leftovers",
        json!({
            "name": "Obsidian",
            "id": "appimage:/x/Obsidian.AppImage",
            "paths": [mine.to_string_lossy(), other.to_string_lossy(), docs.to_string_lossy(), "/etc/passwd", "../../etc"]
        }),
    )
    .unwrap();
    let results = r["results"].as_array().unwrap();
    assert_eq!(results[0]["ok"], true);
    for x in &results[1..] {
        assert_eq!(x["ok"], false);
        assert!(x["error"].as_str().unwrap().starts_with("refused"), "{x}");
    }
    assert!(!mine.exists());
    assert!(other.exists());
    assert!(docs.join("keep.txt").exists());
    assert_eq!(r["totalBytes"], 5);
}

#[test]
fn remove_leftovers_refuses_while_the_app_is_still_installed() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_dpkg(&m);
    let cfg = c.env.config_dir.join("firefox");
    fs::create_dir_all(&cfg).unwrap();
    for (name, id) in [
        ("Firefox", "dpkg:something-else"),
        ("whatever", "dpkg:firefox"),
    ] {
        let e = call(
            &c,
            "uninstall.remove_leftovers",
            json!({"name": name, "id": id, "paths": [cfg.to_string_lossy()]}),
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams, "{name}");
        assert!(e.message.contains("still installed"), "{}", e.message);
    }
    assert!(cfg.exists());
    // validation
    let e = call(
        &c,
        "uninstall.remove_leftovers",
        json!({"name": "x", "id": "y", "paths": []}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    let e = call(&c, "uninstall.leftovers", json!({"name": "  ", "id": "y"})).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
}

// ---------------------------------------------------------------- Windows

struct FakeReg(Vec<RegEntry>);
impl UninstallRegistry for FakeReg {
    fn entries(&self) -> Result<Vec<RegEntry>> {
        Ok(self.0.clone())
    }
}

const GUID: &str = "{90160000-008C-0000-0000-0000000FF1CE}";

fn win_entries() -> Vec<RegEntry> {
    vec![
        RegEntry::new(Hive::Hkcu, "MyTool")
            .with_str("DisplayName", "My Tool")
            .with_str("DisplayVersion", "1.0")
            .with_str(
                "UninstallString",
                r#""C:\Users\u\AppData\Local\MyTool\unins000.exe""#,
            )
            .with_str(
                "QuietUninstallString",
                r#""C:\Users\u\AppData\Local\MyTool\unins000.exe" /VERYSILENT"#,
            ),
        RegEntry::new(Hive::Hklm32, GUID)
            .with_str("DisplayName", "Office")
            .with_str("UninstallString", &format!("MsiExec.exe /I{GUID}"))
            .with_dword("WindowsInstaller", 1),
        RegEntry::new(Hive::Hklm64, "Sys")
            .with_str("DisplayName", "Windows Component")
            .with_dword("SystemComponent", 1)
            .with_str("UninstallString", "rundll32.exe x"),
        RegEntry::new(Hive::Hklm64, "Locked")
            .with_str("DisplayName", "Locked")
            .with_dword("NoRemove", 1)
            .with_str("UninstallString", "x.exe"),
    ]
}

#[test]
fn windows_list_uses_the_registry_and_fields() {
    let (_d, c, _m) = ctx_for(Os::Windows);
    let v = with_registry(FakeReg(win_entries()), || {
        call(&c, "uninstall.list", Value::Null)
    })
    .unwrap();
    let a = v.as_array().unwrap();
    assert_eq!(a.len(), 4);
    let office = a.iter().find(|e| e["name"] == "Office").unwrap();
    assert_eq!(office["canRepair"], true);
    assert_eq!(office["source"], "windows");
    assert_eq!(office["id"], format!("windows:hklm32:{GUID}"));
    let sys = a.iter().find(|e| e["name"] == "Windows Component").unwrap();
    assert_eq!(sys["isSystem"], true);
}

#[test]
fn windows_per_user_uninstall_runs_the_quiet_string_directly() {
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on(
        "C:\\Users\\u\\AppData\\Local\\MyTool\\unins000.exe",
        &["/VERYSILENT"],
        CmdOutput::ok(""),
    );
    let r = with_registry(FakeReg(win_entries()), || {
        with_elevation(false, || {
            call(&c, "uninstall.run", json!({"id": "windows:hkcu:MyTool"}))
        })
    })
    .unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(
        calls(&m),
        ["C:\\Users\\u\\AppData\\Local\\MyTool\\unins000.exe /VERYSILENT"]
    );
}

#[test]
fn windows_machine_wide_uninstall_is_elevated_and_msi_install_switch_becomes_uninstall() {
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on_any_args("powershell", CmdOutput::failed(3010, ""));
    let r = with_registry(FakeReg(win_entries()), || {
        with_elevation(false, || {
            call(
                &c,
                "uninstall.run",
                json!({"id": format!("windows:hklm32:{GUID}")}),
            )
        })
    })
    .unwrap();
    assert_eq!(r["ok"], true, "3010 = success, reboot required: {r}");
    assert_eq!(r["rebootRequired"], true);
    let cs = m.calls();
    assert_eq!(cs.len(), 1);
    let script = powershell_decode(
        &cs[0].1[cs[0].1.iter().position(|a| a == "-EncodedCommand").unwrap() + 1],
    )
    .unwrap();
    assert!(
        script.contains(&format!(r#""MsiExec.exe" "/X{GUID}""#)),
        "{script}"
    );
    assert!(script.contains("-Verb RunAs"));
    // already elevated: no wrapper
    let (_d2, c2, m2) = ctx_for(Os::Windows);
    m2.on("MsiExec.exe", &[&format!("/X{GUID}")], CmdOutput::ok(""));
    let r = with_registry(FakeReg(win_entries()), || {
        with_elevation(true, || {
            call(
                &c2,
                "uninstall.run",
                json!({"id": format!("windows:hklm32:{GUID}")}),
            )
        })
    })
    .unwrap();
    assert_eq!(r["ok"], true);
    assert_eq!(calls(&m2), [format!("MsiExec.exe /X{GUID}")]);
}

#[test]
fn windows_refusals_system_component_and_no_remove() {
    let (_d, c, m) = ctx_for(Os::Windows);
    let run = |id: &str, force: bool| {
        with_registry(FakeReg(win_entries()), || {
            with_elevation(true, || {
                call(&c, "uninstall.run", json!({"id": id, "force": force}))
            })
        })
    };
    assert_eq!(
        run("windows:hklm64:Sys", false).unwrap_err().code,
        ErrorCode::PermissionDenied
    );
    // NoRemove entries cannot be uninstalled at all, force or not
    assert_eq!(
        run("windows:hklm64:Locked", true).unwrap_err().code,
        ErrorCode::InvalidParams
    );
    assert!(m.calls().is_empty());
}

#[test]
fn windows_repair_command_lines() {
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on("msiexec", &["/fa", GUID], CmdOutput::ok(""));
    let r = with_registry(FakeReg(win_entries()), || {
        with_elevation(true, || {
            call(
                &c,
                "uninstall.repair",
                json!({"id": format!("windows:hklm32:{GUID}")}),
            )
        })
    })
    .unwrap();
    assert_eq!(r["ok"], true);
    assert_eq!(calls(&m), [format!("msiexec /fa {GUID}")]);
    // an entry without repair support is refused
    let e = with_registry(FakeReg(win_entries()), || {
        with_elevation(true, || {
            call(&c, "uninstall.repair", json!({"id": "windows:hkcu:MyTool"}))
        })
    })
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
}

#[test]
fn windows_remove_entry_backs_up_first_then_deletes() {
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on_any_args(
        "reg",
        CmdOutput::ok("The operation completed successfully.\n"),
    );
    let r = with_registry(FakeReg(win_entries()), || {
        with_elevation(true, || {
            call(
                &c,
                "uninstall.remove_entry",
                json!({"id": "windows:hkcu:MyTool"}),
            )
        })
    })
    .unwrap();
    assert_eq!(r["ok"], true, "{r}");
    let cs = calls(&m);
    assert_eq!(cs.len(), 2, "{cs:?}");
    let backup = r["backupPath"].as_str().unwrap();
    assert!(
        backup.contains("backups") && backup.contains("uninstall-") && backup.ends_with(".reg"),
        "{backup}"
    );
    assert_eq!(
        cs[0],
        format!(
            r"reg export HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\MyTool {backup} /y"
        )
    );
    assert_eq!(
        cs[1],
        r"reg delete HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\MyTool /f"
    );
    assert!(Path::new(backup).parent().unwrap().exists());
}

#[test]
fn windows_remove_entry_does_not_delete_when_backup_fails() {
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on_any_args("reg", CmdOutput::failed(1, "ERROR: Access is denied."));
    let e = with_registry(FakeReg(win_entries()), || {
        with_elevation(true, || {
            call(
                &c,
                "uninstall.remove_entry",
                json!({"id": "windows:hkcu:MyTool"}),
            )
        })
    })
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Io);
    assert!(e.message.contains("back up"), "{}", e.message);
    assert_eq!(calls(&m).len(), 1, "only the export was attempted");
}

#[test]
fn windows_machine_wide_entry_delete_is_elevated() {
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on_any_args("reg", CmdOutput::ok(""));
    m.on_any_args("powershell", CmdOutput::ok(""));
    let r = with_registry(FakeReg(win_entries()), || {
        with_elevation(false, || {
            call(
                &c,
                "uninstall.remove_entry",
                json!({"id": "windows:hklm64:Sys"}),
            )
        })
    })
    .unwrap();
    assert_eq!(r["ok"], true);
    let cs = m.calls();
    assert_eq!(cs[0].0, "reg", "the export needs no rights");
    assert_eq!(
        cs[1].0, "powershell",
        "the delete goes through the elevation wrapper"
    );
    let script = powershell_decode(
        &cs[1].1[cs[1].1.iter().position(|a| a == "-EncodedCommand").unwrap() + 1],
    )
    .unwrap();
    assert!(
        script.contains(
            r#""reg" "delete" "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Sys" "/f""#
        ),
        "{script}"
    );
}

#[test]
fn windows_rename_entry_validates_name_and_runs_reg_add_after_backup() {
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on_any_args("reg", CmdOutput::ok(""));
    for bad in ["", "   ", "a\"b", "50%", "x\ny"] {
        let e = with_registry(FakeReg(win_entries()), || {
            with_elevation(true, || {
                call(
                    &c,
                    "uninstall.rename_entry",
                    json!({"id": "windows:hkcu:MyTool", "name": bad}),
                )
            })
        })
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams, "{bad:?}");
    }
    assert!(m.calls().is_empty());
    let r = with_registry(FakeReg(win_entries()), || {
        with_elevation(true, || {
            call(
                &c,
                "uninstall.rename_entry",
                json!({"id": "windows:hkcu:MyTool", "name": "  Renamed & Co  "}),
            )
        })
    })
    .unwrap();
    assert_eq!(r["newName"], "Renamed & Co");
    let cs = calls(&m);
    assert!(cs[0].starts_with("reg export "));
    assert_eq!(
        cs[1],
        r"reg add HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\MyTool /v DisplayName /t REG_SZ /d Renamed & Co /f"
    );
}

#[test]
fn windows_only_methods_are_unsupported_elsewhere() {
    for os in [Os::Linux, Os::MacOs] {
        let (_d, c, m) = ctx_for(os);
        for (method, p) in [
            ("uninstall.repair", json!({"id": "x"})),
            ("uninstall.remove_entry", json!({"id": "x"})),
            ("uninstall.rename_entry", json!({"id": "x", "name": "y"})),
        ] {
            let e = call(&c, method, p).unwrap_err();
            assert_eq!(e.code, ErrorCode::Unsupported, "{method} on {os:?}");
        }
        assert!(m.calls().is_empty());
    }
}

// ---------------------------------------------------------------- macOS

#[test]
fn macos_app_goes_to_the_trash_through_finder_and_bundle_id_is_reported() {
    let (_d, c, m) = ctx_for(Os::MacOs);
    let apps = c.env.sys_path("/Applications");
    let app = apps.join("Widget.app");
    fs::create_dir_all(app.join("Contents")).unwrap();
    fs::write(
        app.join("Contents/Info.plist"),
        r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleName</key><string>Widget</string><key>CFBundleIdentifier</key><string>org.example.widget</string><key>CFBundleShortVersionString</key><string>2.0</string></dict></plist>"#,
    )
    .unwrap();
    let lib = c.env.home.join("Library");
    fs::create_dir_all(lib.join("Application Support/Widget")).unwrap();
    fs::create_dir_all(lib.join("Preferences")).unwrap();
    fs::write(lib.join("Preferences/org.example.widget.plist"), "x").unwrap();
    let script = super::macos::trash_script(&app.canonicalize().unwrap());
    m.on("osascript", &["-e", &script], CmdOutput::ok("ok"));
    let id = format!("macapp:{}", app.display());
    let l = call(&c, "uninstall.list", Value::Null).unwrap();
    assert_eq!(l[0]["name"], "Widget");
    let r = call(&c, "uninstall.run", json!({"id": id})).unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["bundleId"], "org.example.widget");
    let paths: Vec<String> = r["leftovers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(paths.len(), 2, "{paths:?}");
    assert!(paths
        .iter()
        .any(|p| p.ends_with("Application Support/Widget")));
    assert!(paths
        .iter()
        .any(|p| p.ends_with("org.example.widget.plist")));
    assert_eq!(calls(&m).last().unwrap(), &format!("osascript -e {script}"));
}

#[test]
fn macos_user_app_falls_back_to_moving_into_the_trash_when_finder_fails() {
    let (_d, c, m) = ctx_for(Os::MacOs);
    let app = c.env.home.join("Applications/Mine.app");
    fs::create_dir_all(app.join("Contents")).unwrap();
    m.on_any_args(
        "osascript",
        CmdOutput::failed(1, "execution error: Finder got an error"),
    );
    let r = call(
        &c,
        "uninstall.run",
        json!({"id": format!("macapp:{}", app.display())}),
    )
    .unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert!(!app.exists());
    assert!(c.env.home.join(".Trash/Mine.app").exists());
}

#[test]
fn macos_system_apps_need_force_and_failed_finder_for_system_folder_is_reported() {
    let (_d, c, m) = ctx_for(Os::MacOs);
    let app = c.env.sys_path("/Applications/Safari.app");
    fs::create_dir_all(app.join("Contents")).unwrap();
    fs::write(
        app.join("Contents/Info.plist"),
        r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.apple.Safari</string></dict></plist>"#,
    )
    .unwrap();
    let id = format!("macapp:{}", app.display());
    let e = call(&c, "uninstall.run", json!({"id": id})).unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    m.on_any_args(
        "osascript",
        CmdOutput::failed(1, "execution error: not allowed (-1)"),
    );
    let r = call(&c, "uninstall.run", json!({"id": id, "force": true})).unwrap();
    assert_eq!(r["ok"], false);
    assert!(app.exists());
}

#[test]
fn macos_brew_and_apps_are_both_listed() {
    let (_d, c, m) = ctx_for(Os::MacOs);
    m.on(
        "brew",
        &["list", "--versions"],
        CmdOutput::ok("wget 1.24.5\n"),
    );
    m.on(
        "brew",
        &["list", "--cask", "--versions"],
        CmdOutput::ok("firefox 126.0\n"),
    );
    let v = call(&c, "uninstall.list", Value::Null).unwrap();
    let ids: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["brew-cask:firefox", "brew:wget"]);
}
