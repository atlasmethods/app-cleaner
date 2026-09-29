//! Handler-level tests: exact command lines, elevation, ignore handling, exit-code quirks.

use serde_json::{json, Value};
use std::fs;
use std::sync::{Arc, Mutex};

use super::*;
use crate::api::dispatch;
use crate::elevate::{powershell_decode, with_elevation};
use crate::job::CancelToken;
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

const APT: &str = "\
Listing...
firefox/noble-updates,noble-security 126.0 amd64 [upgradable from: 125.0]
vim/noble-updates 2:9.1 amd64 [upgradable from: 2:9.0]
htop/noble 3.3.0 amd64 [upgradable from: 3.2.0]
";

fn script_apt(m: &MockRunner) {
    m.on("apt", &["list", "--upgradable"], CmdOutput::ok(APT));
}

fn ids(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn all_methods_are_registered() {
    for m in METHODS {
        assert!(crate::api::registry().get(m).is_some(), "{m}");
    }
}

// ---------------------------------------------------------------- list

#[test]
fn list_apt_shape_and_only_installed_tools_are_queried() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    let v = call(&c, "software_updater.list", Value::Null).unwrap();
    assert_eq!(ids(&v), ["apt:firefox", "apt:htop", "apt:vim"]);
    let ff = &v[0];
    assert_eq!(ff["name"], "firefox");
    assert_eq!(ff["currentVersion"], "125.0");
    assert_eq!(ff["newVersion"], "126.0");
    assert_eq!(ff["source"], "apt");
    assert_eq!(ff["ignored"], false);
    assert_eq!(ff["security"], true);
    assert_eq!(v[2]["security"], false);
    assert_eq!(calls(&m), ["apt list --upgradable"]);
}

#[test]
fn list_marks_ignored_from_settings() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    call(
        &c,
        "software_updater.set_ignored",
        json!({"id": "apt:htop", "ignored": true}),
    )
    .unwrap();
    let v = call(&c, "software_updater.list", json!({})).unwrap();
    let flags: Vec<(String, bool)> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap().to_string(),
                e["ignored"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        flags,
        [
            ("apt:firefox".into(), false),
            ("apt:htop".into(), true),
            ("apt:vim".into(), false)
        ]
    );
}

#[test]
fn list_combines_every_linux_source() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    m.on(
        "dnf",
        &["check-update"],
        CmdOutput {
            status: 100,
            stdout: "Last metadata expiration check: 0:01:00 ago.\n\nkernel-core.x86_64   6.8.9-300.fc40   updates\n".into(),
            stderr: String::new(),
        },
    );
    m.on(
        "rpm",
        &[
            "-q",
            "--queryformat",
            "%{NAME}.%{ARCH}\\t%{VERSION}-%{RELEASE}\\n",
            "kernel-core",
        ],
        CmdOutput::ok("kernel-core.x86_64\t6.8.5-301.fc40\n"),
    );
    m.on(
        "checkupdates",
        &[],
        CmdOutput::ok("linux 6.8.9.arch1-1 -> 6.8.10.arch1-1\n"),
    );
    m.on(
        "flatpak",
        &[
            "remote-ls",
            "--updates",
            "--app",
            "--columns=application,version",
        ],
        CmdOutput::ok("org.mozilla.firefox\t127.0\n"),
    );
    m.on(
        "flatpak",
        &["list", "--app", "--columns=application,name,version"],
        CmdOutput::ok("org.mozilla.firefox\tFirefox\t126.0\n"),
    );
    m.on(
        "snap",
        &["refresh", "--list"],
        CmdOutput::ok("Name Version Rev Size Publisher Notes\nvlc 3.0.21 200 300MB videolan** -\n"),
    );
    m.on(
        "snap",
        &["list"],
        CmdOutput::ok("Name Version Rev Tracking Publisher Notes\nvlc 3.0.20 190 latest/stable videolan** -\n"),
    );
    m.on("brew", &["outdated", "--json=v2"], CmdOutput::ok(r#"{"formulae":[{"name":"wget","installed_versions":["1.0"],"current_version":"1.1"}],"casks":[]}"#));
    m.with_program("pacman");
    let v = call(&c, "software_updater.list", Value::Null).unwrap();
    let a = v.as_array().unwrap();
    let get = |id: &str| {
        a.iter()
            .find(|e| e["id"] == id)
            .unwrap_or_else(|| panic!("{id} in {v}"))
    };
    assert_eq!(
        get("dnf:kernel-core.x86_64")["currentVersion"],
        "6.8.5-301.fc40"
    );
    assert_eq!(get("dnf:kernel-core.x86_64")["name"], "kernel-core");
    assert_eq!(get("pacman:linux")["newVersion"], "6.8.10.arch1-1");
    assert_eq!(get("flatpak:org.mozilla.firefox")["name"], "Firefox");
    assert_eq!(
        get("flatpak:org.mozilla.firefox")["currentVersion"],
        "126.0"
    );
    assert_eq!(get("snap:vlc")["currentVersion"], "3.0.20");
    assert_eq!(get("brew:wget")["source"], "brew");
    assert_eq!(a.len(), 3 + 1 + 1 + 1 + 1 + 1);
}

#[test]
fn dnf_exit_codes_100_updates_0_none_1_error() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "dnf",
        &["check-update"],
        CmdOutput {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
        },
    );
    assert_eq!(
        call(&c, "software_updater.list", Value::Null).unwrap(),
        json!([])
    );

    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "dnf",
        &["check-update"],
        CmdOutput::failed(1, "Error: Failed to download metadata for repo 'updates'"),
    );
    let e = call(&c, "software_updater.list", Value::Null).unwrap_err();
    assert_eq!(e.code, ErrorCode::Io);
    assert!(
        e.message.contains("Failed to download metadata"),
        "{}",
        e.message
    );
}

#[test]
fn one_failing_manager_does_not_hide_the_others() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    m.on(
        "snap",
        &["refresh", "--list"],
        CmdOutput::failed(1, "error: cannot communicate with server"),
    );
    let v = call(&c, "software_updater.list", Value::Null).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 3);
}

#[test]
fn pacman_falls_back_to_qu_and_treats_exit_1_with_no_output_as_none() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on("pacman", &["-Qu"], CmdOutput::failed(1, ""));
    assert_eq!(
        call(&c, "software_updater.list", Value::Null).unwrap(),
        json!([])
    );

    let (_d, c, m) = ctx_for(Os::Linux);
    m.on("pacman", &["-Qu"], CmdOutput::ok("linux 1 -> 2\n"));
    assert_eq!(
        ids(&call(&c, "software_updater.list", Value::Null).unwrap()),
        ["pacman:linux"]
    );

    // checkupdates: exit 2 = nothing to do
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on("checkupdates", &[], CmdOutput::failed(2, ""));
    m.with_program("pacman");
    assert_eq!(
        call(&c, "software_updater.list", Value::Null).unwrap(),
        json!([])
    );
}

#[test]
fn no_package_manager_is_unsupported() {
    let (_d, c, _m) = ctx_for(Os::Linux);
    let e = call(&c, "software_updater.list", Value::Null).unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    let (_d, c, _m) = ctx_for(Os::Windows);
    assert_eq!(
        call(&c, "software_updater.list", Value::Null)
            .unwrap_err()
            .code,
        ErrorCode::Unsupported
    );
}

#[test]
fn refresh_runs_index_updates_elevated_where_needed() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    m.on("dnf", &["check-update"], CmdOutput::ok(""));
    m.on("brew", &["outdated", "--json=v2"], CmdOutput::ok("{}"));
    m.on("pkexec", &["apt-get", "update"], CmdOutput::ok(""));
    m.on("pkexec", &["dnf", "makecache"], CmdOutput::ok(""));
    m.on("brew", &["update"], CmdOutput::ok(""));
    with_elevation(false, || {
        call(&c, "software_updater.list", json!({"refresh": true}))
    })
    .unwrap();
    let cs = calls(&m);
    assert_eq!(
        &cs[..3],
        [
            "pkexec apt-get update",
            "pkexec dnf makecache",
            "brew update"
        ]
    );
    assert!(cs.contains(&"apt list --upgradable".to_string()));

    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    m.on("apt-get", &["update"], CmdOutput::ok(""));
    with_elevation(true, || {
        call(&c, "software_updater.list", json!({"refresh": true}))
    })
    .unwrap();
    assert_eq!(calls(&m)[0], "apt-get update");
}

#[test]
fn refresh_cancelled_authorization_aborts_but_other_refresh_failures_do_not() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    m.on("pkexec", &["apt-get", "update"], CmdOutput::failed(126, ""));
    let e = with_elevation(false, || {
        call(&c, "software_updater.list", json!({"refresh": true}))
    })
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(!calls(&m).contains(&"apt list --upgradable".to_string()));

    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    m.on(
        "apt-get",
        &["update"],
        CmdOutput::failed(100, "E: Failed to fetch"),
    );
    let v = with_elevation(true, || {
        call(&c, "software_updater.list", json!({"refresh": true}))
    })
    .unwrap();
    assert_eq!(v.as_array().unwrap().len(), 3);
}

#[test]
fn windows_list_runs_winget_with_include_unknown() {
    let (_d, c, m) = ctx_for(Os::Windows);
    let table = "Name   Id        Version Available Source\n\
                 ---------------------------------------\n\
                 Git    Git.Git   2.44.0  2.45.1    winget\n\
                 1 upgrades available.\n";
    m.on(
        "winget",
        &["upgrade", "--include-unknown"],
        CmdOutput::ok(table),
    );
    let v = call(&c, "software_updater.list", Value::Null).unwrap();
    assert_eq!(calls(&m), ["winget upgrade --include-unknown"]);
    assert_eq!(v[0]["id"], "winget:Git.Git");
    assert_eq!(v[0]["source"], "winget");
    assert_eq!(v[0]["currentVersion"], "2.44.0");
    assert_eq!(v[0]["newVersion"], "2.45.1");
    assert!(v[0].get("security").is_none());
    // nothing to upgrade: winget prints a sentence (exit code non-zero on some versions)
    let (_d, c, m) = ctx_for(Os::Windows);
    m.on(
        "winget",
        &["upgrade", "--include-unknown"],
        CmdOutput {
            status: 0x8A15_0014u32 as i32,
            stdout: "No installed package found matching input criteria.\n".into(),
            stderr: String::new(),
        },
    );
    assert_eq!(
        call(&c, "software_updater.list", Value::Null).unwrap(),
        json!([])
    );
}

#[test]
fn macos_list_combines_brew_and_softwareupdate() {
    let (_d, c, m) = ctx_for(Os::MacOs);
    m.on("brew", &["outdated", "--json=v2"], CmdOutput::ok(r#"{"formulae":[],"casks":[{"name":"firefox","installed_versions":["125.0"],"current_version":"126.0"}]}"#));
    m.on(
        "softwareupdate",
        &["-l"],
        CmdOutput {
            status: 0,
            stdout: String::new(),
            // recent macOS versions print the report on stderr
            stderr: "Software Update Tool\n\nFinding available software\nSoftware Update found the following new or updated software:\n* Label: Safari17.5-17.5\n\tTitle: Safari, Version: 17.5, Size: 1KiB, Recommended: YES, \n".into(),
        },
    );
    let v = call(&c, "software_updater.list", Value::Null).unwrap();
    assert_eq!(ids(&v), ["brew-cask:firefox", "macos:Safari17.5-17.5"]);
    assert_eq!(v[0]["source"], "brew-cask");
    assert_eq!(v[1]["newVersion"], "17.5");
}

// ---------------------------------------------------------------- update

#[test]
fn update_apt_batches_one_elevated_command() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    m.on(
        "pkexec",
        &[
            "apt-get",
            "install",
            "--only-upgrade",
            "-y",
            "firefox",
            "vim",
        ],
        CmdOutput::ok("Setting up firefox\n"),
    );
    let r = with_elevation(false, || {
        call(
            &c,
            "software_updater.update",
            json!({"ids": ["apt:firefox", "apt:vim"]}),
        )
    })
    .unwrap();
    assert_eq!(
        calls(&m).last().unwrap(),
        "pkexec apt-get install --only-upgrade -y firefox vim"
    );
    assert_eq!(r["succeeded"], 2);
    assert_eq!(r["failed"], 0);
    let res = r["results"].as_array().unwrap();
    assert_eq!(res[0]["id"], "apt:firefox");
    assert_eq!(res[0]["ok"], true);
    assert_eq!(res[0]["exitCode"], 0);
    // elevated: no wrapper
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    m.on(
        "apt-get",
        &["install", "--only-upgrade", "-y", "htop"],
        CmdOutput::ok(""),
    );
    with_elevation(true, || {
        call(&c, "software_updater.update", json!({"ids": ["apt:htop"]}))
    })
    .unwrap();
    assert_eq!(
        calls(&m).last().unwrap(),
        "apt-get install --only-upgrade -y htop"
    );
}

#[test]
fn update_failure_reports_exit_code_and_stderr_summary() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    m.on(
        "apt-get",
        &["install", "--only-upgrade", "-y", "htop"],
        CmdOutput::failed(
            100,
            "E: Unable to fetch some archives\nE: dpkg was interrupted",
        ),
    );
    let r = with_elevation(true, || {
        call(&c, "software_updater.update", json!({"ids": ["apt:htop"]}))
    })
    .unwrap();
    assert_eq!(r["failed"], 1);
    let x = &r["results"][0];
    assert_eq!(x["ok"], false);
    assert_eq!(x["exitCode"], 100);
    let msg = x["message"].as_str().unwrap();
    assert!(
        msg.contains("exit code 100") && msg.contains("dpkg was interrupted"),
        "{msg}"
    );
}

#[test]
fn update_only_accepts_ids_that_are_actually_available() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    m.on(
        "apt-get",
        &["install", "--only-upgrade", "-y", "htop"],
        CmdOutput::ok(""),
    );
    let r = with_elevation(true, || {
        call(
            &c,
            "software_updater.update",
            json!({"ids": ["apt:htop", "apt:evil-package", "apt:x; rm -rf /", "apt:-y", "winget:Foo.Bar"]}),
        )
    })
    .unwrap();
    let res = r["results"].as_array().unwrap();
    assert_eq!(res.len(), 5);
    assert_eq!(r["succeeded"], 1);
    for x in res.iter().filter(|x| x["id"] != "apt:htop") {
        assert_eq!(x["ok"], false);
        assert!(x["message"].as_str().unwrap().starts_with("Refused"), "{x}");
    }
    // the only command that ran installed exactly htop
    let installs: Vec<String> = calls(&m)
        .into_iter()
        .filter(|c| c.contains("install"))
        .collect();
    assert_eq!(installs, ["apt-get install --only-upgrade -y htop"]);
    // validation
    let e = call(&c, "software_updater.update", json!({"ids": []})).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    let e = call(&c, "software_updater.update", json!({})).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
}

#[test]
fn update_all_skips_ignored() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    call(
        &c,
        "software_updater.set_ignored",
        json!({"id": "apt:vim", "ignored": true}),
    )
    .unwrap();
    m.on(
        "apt-get",
        &["install", "--only-upgrade", "-y", "firefox", "htop"],
        CmdOutput::ok(""),
    );
    let r = with_elevation(true, || {
        call(&c, "software_updater.update_all", Value::Null)
    })
    .unwrap();
    assert_eq!(r["succeeded"], 2);
    assert!(r["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|x| x["id"] != "apt:vim"));
    // an explicit selection may still include an ignored item
    m.on(
        "apt-get",
        &["install", "--only-upgrade", "-y", "vim"],
        CmdOutput::ok(""),
    );
    let r = with_elevation(true, || {
        call(&c, "software_updater.update", json!({"ids": ["apt:vim"]}))
    })
    .unwrap();
    assert_eq!(r["succeeded"], 1);
}

#[test]
fn update_all_with_nothing_to_do_runs_nothing() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "apt",
        &["list", "--upgradable"],
        CmdOutput::ok("Listing...\n"),
    );
    let r = with_elevation(true, || {
        call(&c, "software_updater.update_all", Value::Null)
    })
    .unwrap();
    assert_eq!(r["results"], json!([]));
    assert_eq!(calls(&m), ["apt list --upgradable"]);
}

#[test]
fn update_cancelled_authorization_stops_remaining_steps() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    m.on(
        "snap",
        &["refresh", "--list"],
        CmdOutput::ok("Name Version Rev Size Publisher Notes\nvlc 3 1 1MB x -\n"),
    );
    m.on(
        "snap",
        &["list"],
        CmdOutput::ok("Name Version Rev Tracking Publisher Notes\nvlc 2 1 latest x -\n"),
    );
    m.on(
        "pkexec",
        &["apt-get", "install", "--only-upgrade", "-y", "htop"],
        CmdOutput::failed(126, ""),
    );
    let r = with_elevation(false, || {
        call(
            &c,
            "software_updater.update",
            json!({"ids": ["apt:htop", "snap:vlc"]}),
        )
    })
    .unwrap();
    let res = r["results"].as_array().unwrap();
    assert_eq!(r["failed"], 2);
    assert!(res.iter().all(|x| x["ok"] == false));
    assert_eq!(res[0]["message"], "Authorization was cancelled");
    assert!(
        res[1]["message"].as_str().unwrap().starts_with("Not run"),
        "{}",
        res[1]["message"]
    );
    assert!(!calls(&m).iter().any(|c| c.contains("snap refresh vlc")));
}

#[test]
fn update_per_source_command_lines() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "dnf",
        &["check-update"],
        CmdOutput {
            status: 100,
            stdout:
                "firefox.x86_64 126.0-1.fc40 updates\nkernel-core.x86_64 6.8.9-1.fc40 updates\n"
                    .into(),
            stderr: String::new(),
        },
    );
    m.on(
        "flatpak",
        &[
            "remote-ls",
            "--updates",
            "--app",
            "--columns=application,version",
        ],
        CmdOutput::ok("org.gimp.GIMP\t2.10.38\norg.mozilla.firefox\t127.0\n"),
    );
    m.on(
        "snap",
        &["refresh", "--list"],
        CmdOutput::ok("Name Version Rev Size Publisher Notes\nvlc 3 1 1MB x -\n"),
    );
    m.on("brew", &["outdated", "--json=v2"], CmdOutput::ok(r#"{"formulae":[{"name":"wget","installed_versions":["1"],"current_version":"2"}],"casks":[{"name":"iterm2","installed_versions":"3.5.1","current_version":"3.5.2"}]}"#));
    m.on(
        "dnf",
        &["upgrade", "-y", "firefox.x86_64", "kernel-core.x86_64"],
        CmdOutput::ok(""),
    );
    m.on(
        "flatpak",
        &["update", "-y", "org.gimp.GIMP", "org.mozilla.firefox"],
        CmdOutput::ok(""),
    );
    m.on("snap", &["refresh", "vlc"], CmdOutput::ok(""));
    m.on("brew", &["upgrade", "wget"], CmdOutput::ok(""));
    m.on("brew", &["upgrade", "--cask", "iterm2"], CmdOutput::ok(""));
    let r = with_elevation(true, || {
        call(&c, "software_updater.update_all", Value::Null)
    })
    .unwrap();
    assert_eq!(r["failed"], 0, "{r}");
    assert_eq!(r["succeeded"], 7);
    let cs = calls(&m);
    for expected in [
        "dnf upgrade -y firefox.x86_64 kernel-core.x86_64",
        "flatpak update -y org.gimp.GIMP org.mozilla.firefox",
        "snap refresh vlc",
        "brew upgrade wget",
        "brew upgrade --cask iterm2",
    ] {
        assert!(
            cs.contains(&expected.to_string()),
            "{expected} not in {cs:?}"
        );
    }
}

#[test]
fn pacman_update_is_a_full_upgrade_excluding_ignored_packages() {
    let (_d, c, m) = ctx_for(Os::Linux);
    m.on(
        "checkupdates",
        &[],
        CmdOutput::ok("linux 1 -> 2\nfirefox 1 -> 2\nvim 1 -> 2\n"),
    );
    m.with_program("pacman");
    call(
        &c,
        "software_updater.set_ignored",
        json!({"id": "pacman:vim", "ignored": true}),
    )
    .unwrap();
    m.on(
        "pacman",
        &["-Syu", "--noconfirm", "--ignore", "vim"],
        CmdOutput::ok(""),
    );
    let r = with_elevation(true, || {
        call(
            &c,
            "software_updater.update",
            json!({"ids": ["pacman:firefox"]}),
        )
    })
    .unwrap();
    assert_eq!(
        calls(&m).last().unwrap(),
        "pacman -Syu --noconfirm --ignore vim"
    );
    let res = r["results"].as_array().unwrap();
    // every non-ignored pending package is reported, since all of them were upgraded
    let got: Vec<&str> = res.iter().map(|x| x["id"].as_str().unwrap()).collect();
    assert_eq!(got, ["pacman:firefox", "pacman:linux"]);
    assert!(res[0]["message"]
        .as_str()
        .unwrap()
        .contains("full system upgrade"));
}

#[test]
fn winget_update_command_line_and_truncated_ids_are_refused() {
    let (_d, c, m) = ctx_for(Os::Windows);
    let table = "Name   Id                       Version Available Source\n\
                 ----------------------------------------------------\n\
                 Git    Git.Git                  2.44.0  2.45.1    winget\n\
                 VS     Microsoft.VisualStudio.… 17.9    17.10     winget\n";
    m.on(
        "winget",
        &["upgrade", "--include-unknown"],
        CmdOutput::ok(table),
    );
    m.on(
        "winget",
        &[
            "upgrade",
            "--id",
            "Git.Git",
            "-e",
            "--silent",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ],
        CmdOutput::ok("Successfully installed"),
    );
    let r = call(&c, "software_updater.update_all", Value::Null).unwrap();
    assert_eq!(
        calls(&m).last().unwrap(),
        "winget upgrade --id Git.Git -e --silent --accept-package-agreements --accept-source-agreements"
    );
    let res = r["results"].as_array().unwrap();
    assert_eq!(res.len(), 2);
    let git = res.iter().find(|x| x["id"] == "winget:Git.Git").unwrap();
    assert_eq!(git["ok"], true);
    let vs = res
        .iter()
        .find(|x| x["id"].as_str().unwrap().starts_with("winget:Microsoft"))
        .unwrap();
    assert_eq!(vs["ok"], false);
    assert!(
        vs["message"].as_str().unwrap().contains("truncated"),
        "{vs}"
    );
}

#[test]
fn macos_system_update_is_elevated_with_a_safely_quoted_label() {
    let (_d, c, m) = ctx_for(Os::MacOs);
    m.on(
        "softwareupdate",
        &["-l"],
        CmdOutput::ok("* Label: macOS Sonoma 14.5-23F79\n\tTitle: macOS Sonoma 14.5, Version: 14.5, Size: 1KiB, Recommended: YES, Action: restart, \n"),
    );
    let script =
        crate::elevate::macos_admin_script("softwareupdate", &["-i", "macOS Sonoma 14.5-23F79"]);
    m.on("osascript", &["-e", &script], CmdOutput::ok("Done."));
    let r = with_elevation(false, || {
        call(
            &c,
            "software_updater.update",
            json!({"ids": ["macos:macOS Sonoma 14.5-23F79"]}),
        )
    })
    .unwrap();
    assert_eq!(r["succeeded"], 1, "{r}");
    assert_eq!(
        script,
        "do shell script \"softwareupdate -i 'macOS Sonoma 14.5-23F79' 2>&1\" with administrator privileges"
    );
}

#[test]
fn windows_elevation_wrapper_is_not_used_for_winget() {
    // winget elevates itself per package where needed; we never wrap it.
    let (_d, c, m) = ctx_for(Os::Windows);
    let table = "Name Id      Version Available Source\n\
                 ------------------------------------\n\
                 Git  Git.Git 1       2         winget\n";
    m.on(
        "winget",
        &["upgrade", "--include-unknown"],
        CmdOutput::ok(table),
    );
    m.on_any_args("winget", CmdOutput::ok(table));
    let r = with_elevation(false, || {
        call(
            &c,
            "software_updater.update",
            json!({"ids": ["winget:Git.Git"]}),
        )
    })
    .unwrap();
    assert_eq!(r["succeeded"], 1);
    assert!(calls(&m).iter().all(|c| !c.starts_with("powershell")));
    let _ = powershell_decode;
}

#[test]
fn update_emits_progress_per_step_and_honours_cancel() {
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    m.on(
        "snap",
        &["refresh", "--list"],
        CmdOutput::ok("Name Version Rev Size Publisher Notes\nvlc 3 1 1MB x -\n"),
    );
    m.on(
        "apt-get",
        &["install", "--only-upgrade", "-y", "htop"],
        CmdOutput::ok(""),
    );
    m.on("snap", &["refresh", "vlc"], CmdOutput::ok(""));
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let job = Job::new(CancelToken::new(), move |e| sink.lock().unwrap().push(e));
    with_elevation(true, || {
        dispatch(
            &c,
            "software_updater.update",
            json!({"ids": ["apt:htop", "snap:vlc"]}),
            &job,
        )
    })
    .unwrap();
    let ev = events.lock().unwrap();
    let upd: Vec<_> = ev.iter().filter(|e| e.stage == "update").collect();
    assert!(upd.len() >= 3);
    assert_eq!(upd[0].current, Some(0));
    assert_eq!(upd[0].total, Some(2));
    assert_eq!(upd.last().unwrap().current, Some(2));

    // Cancelled before the second step: it is reported as not run and nothing else executes.
    let (_d, c, m) = ctx_for(Os::Linux);
    script_apt(&m);
    m.on(
        "apt-get",
        &["install", "--only-upgrade", "-y", "htop"],
        CmdOutput::ok(""),
    );
    let token = CancelToken::new();
    let t2 = token.clone();
    let job = Job::new(token, move |e| {
        if e.stage == "update" && e.current == Some(1) {
            t2.cancel();
        }
    });
    m.on(
        "snap",
        &["refresh", "--list"],
        CmdOutput::ok("Name Version Rev Size Publisher Notes\nvlc 3 1 1MB x -\n"),
    );
    let r = with_elevation(true, || {
        dispatch(
            &c,
            "software_updater.update",
            json!({"ids": ["apt:htop", "snap:vlc"]}),
            &job,
        )
    })
    .unwrap();
    let res = r["results"].as_array().unwrap();
    assert_eq!(res.iter().filter(|x| x["ok"] == true).count(), 1);
    assert!(res.iter().any(|x| x["message"] == "Not run: cancelled"));
    assert!(!calls(&m).iter().any(|c| c.contains("snap refresh vlc")));
}

// ---------------------------------------------------------------- set_ignored

#[test]
fn set_ignored_persists_toggles_and_validates() {
    let (_d, c, _m) = ctx_for(Os::Linux);
    let r = call(
        &c,
        "software_updater.set_ignored",
        json!({"id": "apt:vim", "ignored": true}),
    )
    .unwrap();
    assert_eq!(r["ignoredUpdates"], json!(["apt:vim"]));
    call(
        &c,
        "software_updater.set_ignored",
        json!({"id": "apt:vim", "ignored": true}),
    )
    .unwrap();
    call(
        &c,
        "software_updater.set_ignored",
        json!({"id": "snap:vlc", "ignored": true}),
    )
    .unwrap();
    assert_eq!(settings::load(&c).ignored_updates, ["apt:vim", "snap:vlc"]);
    let r = call(
        &c,
        "software_updater.set_ignored",
        json!({"id": "apt:vim", "ignored": false}),
    )
    .unwrap();
    assert_eq!(r["ignoredUpdates"], json!(["snap:vlc"]));
    for bad in [
        json!({"id": "", "ignored": true}),
        json!({"id": "a\nb", "ignored": true}),
        json!({"ignored": true}),
        json!({"id": "x"}),
    ] {
        assert_eq!(
            call(&c, "software_updater.set_ignored", bad)
                .unwrap_err()
                .code,
            ErrorCode::InvalidParams
        );
    }
    // also visible through settings.get
    let s = call(&c, "settings.get", Value::Null).unwrap();
    assert_eq!(s["ignoredUpdates"], json!(["snap:vlc"]));
}
