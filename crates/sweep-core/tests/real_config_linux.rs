//! Real-filesystem, real-process test of the config-issues cleaner: broken launchers, autostart
//! entries, links and `mimeapps.list` in a throwaway HOME are found, fixed, and restored
//! byte-for-byte. `#[ignore]`d like the other `real_` tests:
//!
//! ```text
//! cargo test -p sweep-core -- --ignored real_
//! ```

#![cfg(target_os = "linux")]

use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use sweep_core::{dispatch, Ctx, Env, Job, SystemRunner};

fn call(c: &Ctx, method: &str, params: Value) -> Value {
    dispatch(c, method, params, &Job::detached()).unwrap_or_else(|e| panic!("{method}: {e}"))
}

fn write(p: &Path, s: &str, mode: u32) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o7777
}

#[test]
#[ignore = "real filesystem + real process runner; run with --ignored real_"]
fn real_config_issues_scan_fix_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let env = Env::for_test(tmp.path());
    fs::create_dir_all(&env.home).unwrap();
    fs::create_dir_all(&env.root).unwrap();
    // real runner, sandboxed paths
    let ctx = Ctx::new(env.clone(), Arc::new(SystemRunner));

    let apps = env.user_data_dir.join("applications");
    write(&env.root.join("usr/bin/present"), "#!/bin/sh\n", 0o755);
    write(
        &env.root.join("usr/share/applications/gedit.desktop"),
        "[Desktop Entry]\n",
        0o644,
    );
    let broken = "[Desktop Entry]\nType=Application\nName=Old\nExec=/opt/vanished/app --x %U\n";
    write(&apps.join("old.desktop"), broken, 0o750);
    write(
        &apps.join("fine.desktop"),
        "[Desktop Entry]\nType=Application\nName=Fine\nExec=/usr/bin/present\n",
        0o644,
    );
    write(
        &env.config_dir.join("autostart/old-auto.desktop"),
        broken,
        0o644,
    );
    let mime = "# mine\n[Default Applications]\ntext/plain=gedit.desktop;vanished.desktop;\nx/y=vanished.desktop;\n";
    write(&env.config_dir.join("mimeapps.list"), mime, 0o640);
    fs::create_dir_all(env.home.join(".local/bin")).unwrap();
    symlink("/opt/vanished/tool", env.home.join(".local/bin/dead")).unwrap();

    let cats = json!([
        "desktop_entries",
        "autostart",
        "broken_symlinks",
        "mime_associations"
    ]);
    let scan = call(&ctx, "registry_cleaner.scan", json!({ "categories": cats }));
    let issues = scan["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 5, "{scan}");
    let ids: Vec<Value> = issues.iter().map(|i| i["id"].clone()).collect();

    let fixed = call(
        &ctx,
        "registry_cleaner.fix",
        json!({ "issueIds": ids, "backup": true, "categories": cats }),
    );
    assert_eq!(fixed["fixed"], 5, "{fixed}");
    assert!(!apps.join("old.desktop").exists());
    assert!(apps.join("fine.desktop").exists());
    assert!(!env.config_dir.join("autostart/old-auto.desktop").exists());
    assert!(fs::symlink_metadata(env.home.join(".local/bin/dead")).is_err());
    assert_eq!(
        fs::read_to_string(env.config_dir.join("mimeapps.list")).unwrap(),
        "# mine\n[Default Applications]\ntext/plain=gedit.desktop;\n"
    );
    assert!(
        call(&ctx, "registry_cleaner.scan", json!({ "categories": cats }))["issues"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let id = fixed["backupId"].as_str().unwrap();
    let restored = call(&ctx, "registry_cleaner.restore_backup", json!({ "id": id }));
    assert_eq!(restored["ok"], true, "{restored}");
    assert_eq!(
        fs::read_to_string(apps.join("old.desktop")).unwrap(),
        broken
    );
    assert_eq!(mode(&apps.join("old.desktop")), 0o750);
    assert_eq!(
        fs::read_to_string(env.config_dir.join("mimeapps.list")).unwrap(),
        mime
    );
    assert_eq!(mode(&env.config_dir.join("mimeapps.list")), 0o640);
    assert_eq!(
        fs::read_link(env.home.join(".local/bin/dead")).unwrap(),
        Path::new("/opt/vanished/tool")
    );
    assert_eq!(
        call(&ctx, "registry_cleaner.scan", json!({ "categories": cats }))["issues"]
            .as_array()
            .unwrap()
            .len(),
        5
    );

    call(&ctx, "registry_cleaner.delete_backup", json!({ "id": id }));
    assert!(call(&ctx, "registry_cleaner.list_backups", json!({}))
        .as_array()
        .unwrap()
        .is_empty());
}
