//! Browser plugin tests: Chromium Preferences editing, Firefox extensions.json + mozLz4,
//! running-browser refusal, backups.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::ctx::Env;
use crate::error::ErrorCode;
use crate::procs::FakeProcesses;
use crate::runner::MockRunner;

fn job() -> Job {
    Job::detached()
}

fn ctx(dir: &Path, running: &[&str]) -> Ctx {
    let mut c = Ctx::new(Env::for_test(dir), Arc::new(MockRunner::new()))
        .with_procs(Arc::new(FakeProcesses::new(running, true)));
    c.env.os = Os::Linux;
    c
}

fn call(c: &Ctx, m: &str, p: Value) -> Result<Value> {
    crate::api::dispatch(c, m, p, &job())
}

fn write(p: &Path, text: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

// ---------------------------------------------------------------- Chromium fixture

const ID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"; // in Preferences, new format
const ID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"; // in Preferences, old `state` format, disabled
const ID_C: &str = "cccccccccccccccccccccccccccccccc"; // Secure Preferences only
const ID_D: &str = "dddddddddddddddddddddddddddddddd"; // component (skipped)
const ID_E: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"; // policy
const ID_F: &str = "ffffffffffffffffffffffffffffffff"; // unpacked, absolute path
const ID_G: &str = "gggggggggggggggggggggggggggggggg"; // theme
const ID_H: &str = "hhhhhhhhhhhhhhhhhhhhhhhhhhhhhhhh"; // disabled with array reasons + permissions bit

struct Chrome {
    profile: PathBuf,
}

impl Chrome {
    fn prefs(&self) -> PathBuf {
        self.profile.join("Preferences")
    }
    fn secure(&self) -> PathBuf {
        self.profile.join("Secure Preferences")
    }
    fn ext(&self, id: &str) -> PathBuf {
        self.profile.join("Extensions").join(id)
    }
}

fn manifest(dir: &Path, json: &str) {
    write(&dir.join("manifest.json"), json);
}

fn chrome_fixture(c: &Ctx) -> Chrome {
    let profile = c.env.config_dir.join("google-chrome/Default");
    let ch = Chrome {
        profile: profile.clone(),
    };
    // A: name via __MSG_, key in the messages file with different case
    manifest(
        &ch.ext(ID_A).join("1.2.3_0"),
        r#"{"name":"__MSG_extName__","version":"1.2.3","description":"__MSG_extdesc__","default_locale":"en","manifest_version":3}"#,
    );
    write(
        &ch.ext(ID_A).join("1.2.3_0/_locales/en/messages.json"),
        r#"{"extNAME":{"message":"Ad Blocker Pro"},"EXTDESC":{"message":"Blocks ads"}}"#,
    );
    // B: manifest with comments and a BOM
    manifest(
        &ch.ext(ID_B).join("2.0_0"),
        "\u{feff}{\n // a comment\n \"name\": \"Old Format Ext\", /* inline */ \"version\": \"2.0\"\n}",
    );
    manifest(
        &ch.ext(ID_C).join("3.0_0"),
        r#"{"name":"Protected Ext","version":"3.0"}"#,
    );
    manifest(
        &ch.ext(ID_D).join("1_0"),
        r#"{"name":"Component","version":"1"}"#,
    );
    manifest(
        &ch.ext(ID_E).join("1_0"),
        r#"{"name":"Policy Ext","version":"1"}"#,
    );
    let unpacked = c.env.home.join("dev/my-unpacked");
    manifest(&unpacked, r#"{"name":"My Unpacked","version":"0.1"}"#);
    manifest(
        &ch.ext(ID_G).join("1_0"),
        r#"{"name":"Dark Theme","version":"1","theme":{"colors":{}}}"#,
    );
    manifest(
        &ch.ext(ID_H).join("1_0"),
        r#"{"name":"Perm Ext","version":"1"}"#,
    );

    let prefs = json!({
        "profile": {"name": "Work"},
        "browser": {"window_placement": {"left": 1, "top": 2.5}},
        "extensions": {
            "toolbar": [ID_A],
            "settings": {
                ID_A: {"location": 1, "path": format!("{ID_A}/1.2.3_0"), "state": 1, "from_webstore": true, "install_time": "13300000000000000", "granted_permissions": {"api": ["tabs"]}},
                ID_B: {"location": 1, "path": format!("{ID_B}/2.0_0"), "state": 0},
                ID_G: {"location": 1, "path": format!("{ID_G}/1_0"), "state": 1},
                ID_H: {"location": 1, "path": format!("{ID_H}/1_0"), "disable_reasons": [1, 2]},
                ID_E: {"location": 7, "path": format!("{ID_E}/1_0"), "state": 1},
                ID_F: {"location": 4, "path": unpacked.to_string_lossy(), "state": 1},
                ID_D: {"location": 5, "path": format!("{ID_D}/1_0"), "state": 1},
                "nmmhkkegccagdldgiimedpiccmgmieda": {"location": 1, "path": "nmmhkkegccagdldgiimedpiccmgmieda/1_0", "state": 1}
            }
        }
    });
    write(&ch.prefs(), &prefs.to_string());
    let secure = json!({
        "extensions": {"settings": {
            ID_C: {"location": 1, "path": format!("{ID_C}/3.0_0"), "state": 1}
        }},
        "protection": {"macs": {"extensions": {"settings": {ID_C: "ABCDEF"}}}, "super_mac": "1234"}
    });
    write(&ch.secure(), &secure.to_string());
    ch
}

fn plugin<'a>(v: &'a Value, ext: &str) -> &'a Value {
    v.as_array()
        .unwrap()
        .iter()
        .find(|p| p["extensionId"] == ext)
        .unwrap_or_else(|| panic!("no plugin {ext} in {v}"))
}

fn pid(ext: &str) -> String {
    format!("chrome:Default:{ext}")
}

#[test]
fn chromium_listing_resolves_names_and_skips_builtins() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    chrome_fixture(&c);
    let v = call(&c, "browser_plugins.list", json!({})).unwrap();
    let ids: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["extensionId"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&ID_D), "component extensions are skipped");
    assert!(
        !ids.contains(&"nmmhkkegccagdldgiimedpiccmgmieda"),
        "browser built-ins are skipped"
    );
    assert_eq!(ids.len(), 7, "{ids:?}");

    let a = plugin(&v, ID_A);
    assert_eq!(a["id"], pid(ID_A));
    assert_eq!(a["browser"], "chrome");
    assert_eq!(a["browserLabel"], "Google Chrome");
    assert_eq!(a["profile"], "Default");
    assert_eq!(a["profileName"], "Work");
    assert_eq!(
        a["name"], "Ad Blocker Pro",
        "__MSG_ resolved case-insensitively"
    );
    assert_eq!(a["description"], "Blocks ads");
    assert_eq!(a["version"], "1.2.3");
    assert_eq!(a["type"], "extension");
    assert_eq!(a["enabled"], true);
    assert_eq!(a["canDisable"], true);
    assert_eq!(a["canRemove"], true);
    assert_eq!(a["running"], false);
    assert!(a.get("note").is_none());

    let b = plugin(&v, ID_B);
    assert_eq!(
        b["name"], "Old Format Ext",
        "comments and BOM in manifest.json"
    );
    assert_eq!(b["enabled"], false, "old state=0 format");
    assert_eq!(plugin(&v, ID_H)["enabled"], false, "array disable_reasons");
    assert_eq!(plugin(&v, ID_G)["type"], "theme");

    let cc = plugin(&v, ID_C);
    assert_eq!(cc["enabled"], true);
    assert_eq!(cc["canDisable"], false);
    assert_eq!(
        cc["note"],
        "This browser protects extension settings; disable it from the browser's extensions page"
    );
    assert_eq!(cc["canRemove"], true);

    let e = plugin(&v, ID_E);
    assert_eq!(e["canDisable"], false);
    assert_eq!(e["canRemove"], false);
    let f = plugin(&v, ID_F);
    assert_eq!(f["name"], "My Unpacked");
    assert_eq!(
        f["canRemove"], false,
        "an unpacked folder is the user's own"
    );
    assert_eq!(f["canDisable"], true);
}

#[test]
fn chromium_disable_and_enable_preserve_every_other_key() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let before = read_json(&ch.prefs());
    let secure_bytes = fs::read(ch.secure()).unwrap();

    let r = call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": pid(ID_A), "enabled": false}),
    )
    .unwrap();
    assert_eq!(r["plugin"]["enabled"], false);
    let after = read_json(&ch.prefs());
    let mut expected = before.clone();
    let e = expected["extensions"]["settings"][ID_A]
        .as_object_mut()
        .unwrap();
    e.insert("disable_reasons".into(), json!(1));
    e.insert("state".into(), json!(0));
    assert_eq!(after, expected, "only the entry's state keys changed");
    assert_eq!(
        fs::read(ch.secure()).unwrap(),
        secure_bytes,
        "Secure Preferences never touched"
    );

    let r = call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": pid(ID_A), "enabled": true}),
    )
    .unwrap();
    assert_eq!(r["plugin"]["enabled"], true);
    let mut restored = before.clone();
    restored["extensions"]["settings"][ID_A]["state"] = json!(1);
    assert_eq!(read_json(&ch.prefs()), restored);

    // old format: state 0 -> enable sets state 1; disable_reasons array format is kept as array
    call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": pid(ID_B), "enabled": true}),
    )
    .unwrap();
    assert_eq!(
        read_json(&ch.prefs())["extensions"]["settings"][ID_B]["state"],
        1
    );
    let v = call(&c, "browser_plugins.list", json!({})).unwrap();
    assert_eq!(plugin(&v, ID_B)["enabled"], true);
    call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": pid(ID_B), "enabled": false}),
    )
    .unwrap();
    assert_eq!(
        read_json(&ch.prefs())["extensions"]["settings"][ID_B]["state"],
        0
    );
}

#[test]
fn chromium_array_reasons_and_other_disable_reasons() {
    let mut e = json!({"disable_reasons": [1, 2], "state": 0})
        .as_object()
        .unwrap()
        .clone();
    // still disabled for a permissions reason after removing the user-action bit
    let err = chromium::apply_enabled_to_entry(&mut e, true).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported);
    let mut e = json!({"disable_reasons": [1], "x": 5})
        .as_object()
        .unwrap()
        .clone();
    chromium::apply_enabled_to_entry(&mut e, true).unwrap();
    assert_eq!(Value::Object(e), json!({"x": 5}));
    let mut e = json!({"disable_reasons": [4], "x": 5})
        .as_object()
        .unwrap()
        .clone();
    chromium::apply_enabled_to_entry(&mut e, false).unwrap();
    assert_eq!(e["disable_reasons"], json!([4, 1]), "array format kept");
    let mut e = json!({"disable_reasons": 2}).as_object().unwrap().clone();
    chromium::apply_enabled_to_entry(&mut e, false).unwrap();
    assert_eq!(e["disable_reasons"], json!(3), "bitmask format kept");
}

#[test]
fn chromium_secure_preferences_entries_are_never_edited() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let p_bytes = fs::read(ch.prefs()).unwrap();
    let s_bytes = fs::read(ch.secure()).unwrap();
    let e = call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": pid(ID_C), "enabled": false}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    assert!(e.message.contains("protects extension settings"));
    assert_eq!(fs::read(ch.prefs()).unwrap(), p_bytes);
    assert_eq!(fs::read(ch.secure()).unwrap(), s_bytes);
    // policy-installed too
    assert!(call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": pid(ID_E), "enabled": false})
    )
    .is_err());
    assert_eq!(fs::read(ch.prefs()).unwrap(), p_bytes);
    // no backup folder is left behind by a refused change
    assert!(
        !c.env.data_dir.join("backups").exists()
            || fs::read_dir(c.env.data_dir.join("backups"))
                .unwrap()
                .count()
                == 0
    );
}

#[test]
fn chromium_entry_with_its_own_mac_is_treated_as_protected() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let mut p = read_json(&ch.prefs());
    p["protection"] = json!({"macs": {"extensions": {"settings": {ID_A: "MAC"}}}});
    fs::write(ch.prefs(), p.to_string()).unwrap();
    let v = call(&c, "browser_plugins.list", json!({})).unwrap();
    assert_eq!(plugin(&v, ID_A)["canDisable"], false);
    assert!(call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": pid(ID_A), "enabled": false})
    )
    .is_err());
    assert_eq!(read_json(&ch.prefs()), p);
}

#[test]
fn a_running_browser_blocks_changes_with_a_clear_message() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &["chrome"]);
    let ch = chrome_fixture(&c);
    let before = fs::read(ch.prefs()).unwrap();
    let e = call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": pid(ID_A), "enabled": false}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(
        e.message.starts_with("Close Google Chrome first"),
        "{}",
        e.message
    );
    let e = call(&c, "browser_plugins.remove", json!({"id": pid(ID_A)})).unwrap_err();
    assert!(e.message.starts_with("Close Google Chrome first"));
    assert_eq!(fs::read(ch.prefs()).unwrap(), before);
    assert!(ch.ext(ID_A).exists());
    // listing still works and says so
    let v = call(&c, "browser_plugins.list", json!({})).unwrap();
    assert_eq!(plugin(&v, ID_A)["running"], true);
}

#[test]
fn chromium_backup_is_taken_before_the_change() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let before = fs::read(ch.prefs()).unwrap();
    let r = call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": pid(ID_A), "enabled": false}),
    )
    .unwrap();
    let bid = r["backupId"].as_str().unwrap();
    assert!(bid.starts_with("plugins-"));
    let dir = c.env.data_dir.join("backups").join(bid);
    let m = read_json(&dir.join("manifest.json"));
    assert_eq!(m["kind"], "plugins");
    assert!(m["description"]
        .as_str()
        .unwrap()
        .contains("Ad Blocker Pro"));
    let rel = m["items"][0]["backup"].as_str().unwrap();
    assert_eq!(fs::read(dir.join(rel)).unwrap(), before);
    assert_eq!(
        m["items"][0]["original"],
        ch.prefs().to_string_lossy().as_ref()
    );
}

#[test]
fn a_failing_backup_stops_the_change() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let before = fs::read(ch.prefs()).unwrap();
    fs::create_dir_all(&c.env.data_dir).unwrap();
    fs::write(c.env.data_dir.join("backups"), "not a folder").unwrap();
    assert!(call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": pid(ID_A), "enabled": false})
    )
    .is_err());
    assert!(call(&c, "browser_plugins.remove", json!({"id": pid(ID_A)})).is_err());
    assert_eq!(fs::read(ch.prefs()).unwrap(), before);
    assert!(ch.ext(ID_A).join("1.2.3_0/manifest.json").exists());
}

#[test]
fn chromium_remove_deletes_dir_and_pref_entry_after_backing_up() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let before = read_json(&ch.prefs());
    let r = call(&c, "browser_plugins.remove", json!({"id": pid(ID_A)})).unwrap();
    assert!(!ch.ext(ID_A).exists());
    let after = read_json(&ch.prefs());
    let mut expected = before.clone();
    expected["extensions"]["settings"]
        .as_object_mut()
        .unwrap()
        .remove(ID_A);
    assert_eq!(after, expected);
    let dir = c
        .env
        .data_dir
        .join("backups")
        .join(r["backupId"].as_str().unwrap());
    let m = read_json(&dir.join("manifest.json"));
    let items = m["items"].as_array().unwrap();
    assert_eq!(items[0]["type"], "dir");
    let copy = dir.join(items[0]["backup"].as_str().unwrap());
    assert!(copy.join("1.2.3_0/manifest.json").is_file());
    assert!(copy.join("1.2.3_0/_locales/en/messages.json").is_file());
    let listing = call(&c, "browser_plugins.list", json!({})).unwrap();
    assert!(listing
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["extensionId"] != ID_A));
}

#[test]
fn chromium_remove_of_a_secure_entry_removes_only_the_directory() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let p = fs::read(ch.prefs()).unwrap();
    let s = fs::read(ch.secure()).unwrap();
    let r = call(&c, "browser_plugins.remove", json!({"id": pid(ID_C)})).unwrap();
    assert!(!ch.ext(ID_C).exists());
    assert!(r["note"]
        .as_str()
        .unwrap()
        .contains("clear it the next time"));
    assert_eq!(fs::read(ch.secure()).unwrap(), s);
    assert_eq!(fs::read(ch.prefs()).unwrap(), p);
    // unpacked and policy extensions cannot be removed
    assert!(call(&c, "browser_plugins.remove", json!({"id": pid(ID_F)})).is_err());
    assert!(call(&c, "browser_plugins.remove", json!({"id": pid(ID_E)})).is_err());
    assert!(c.env.home.join("dev/my-unpacked/manifest.json").exists());
}

#[test]
fn ids_cannot_escape_or_select_things_that_do_not_exist() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    for bad in [
        "nonsense",
        "chrome:Default",
        "chrome:Default:../../x",
        "chrome:Nope:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "opera:Default:x",
        "chrome:Default:",
    ] {
        let e = call(&c, "browser_plugins.remove", json!({"id": bad})).unwrap_err();
        assert!(
            matches!(e.code, ErrorCode::InvalidParams | ErrorCode::NotFound),
            "{bad}: {e:?}"
        );
    }
    assert!(ch.ext(ID_A).exists());
}

#[test]
fn several_profiles_and_roots_get_distinct_ids_and_opera_uses_the_root() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let base = c.env.config_dir.join("google-chrome");
    for prof in ["Default", "Profile 1", "Guest Profile", "System Profile"] {
        write(
            &base.join(prof).join("Preferences"),
            &json!({"extensions": {"settings": {}}}).to_string(),
        );
    }
    write(
        &c.env
            .config_dir
            .join("google-chrome-beta/Default/Preferences"),
        "{}",
    );
    let opera = c.env.config_dir.join("opera");
    write(&opera.join("Preferences"), "{}");
    fs::create_dir_all(opera.join("Extensions")).unwrap();
    let mut labels: Vec<String> = discover(&c)
        .iter()
        .map(|p| format!("{}:{}", p.def.key, p.label))
        .collect();
    labels.sort();
    assert_eq!(
        labels,
        vec![
            "chrome:Default",
            "chrome:Default@google-chrome-beta",
            "chrome:Profile 1",
            "opera:Default"
        ]
    );
}

// ---------------------------------------------------------------- Firefox

struct Fox {
    profile: PathBuf,
}

const FX_UBO: &str = "uBlock0@raymondhill.net";
const FX_THEME: &str = "night@example.org";
const FX_BUILTIN: &str = "default-theme@mozilla.org";

fn firefox_fixture(c: &Ctx) -> Fox {
    let root = c.env.home.join(".mozilla/firefox");
    let profile = root.join("abcd1234.default-release");
    write(&root.join("profiles.ini"), "[Profile0]\nName=default-release\nIsRelative=1\nPath=abcd1234.default-release\nDefault=1\n");
    write(&profile.join("prefs.js"), "// prefs\n");
    let db = json!({
        "schemaVersion": 36,
        "addons": [
            {"id": FX_UBO, "type": "extension", "active": true, "userDisabled": false, "appDisabled": false, "location": "app-profile", "version": "1.60.0", "visible": true,
             "defaultLocale": {"name": "uBlock Origin", "description": "Finally, an efficient blocker."}, "signedState": 2, "unknownKey": {"keep": [1, 2, 3]}},
            {"id": FX_THEME, "type": "theme", "active": false, "userDisabled": true, "location": "app-profile", "version": "1.0", "visible": true,
             "defaultLocale": {"name": "Night"}},
            {"id": FX_BUILTIN, "type": "theme", "active": true, "userDisabled": false, "location": "app-builtin", "version": "1.2", "visible": true,
             "defaultLocale": {"name": "System theme"}},
            {"id": "sys@mozilla.org", "type": "extension", "active": true, "userDisabled": false, "location": "app-system-defaults", "version": "1", "visible": true,
             "defaultLocale": {"name": "Sys"}},
            {"id": "hidden@example.org", "type": "extension", "active": true, "userDisabled": false, "location": "app-profile", "version": "1", "visible": false,
             "defaultLocale": {"name": "Hidden"}},
            {"id": "global@example.org", "type": "extension", "active": true, "userDisabled": false, "location": "app-global", "version": "1", "visible": true,
             "defaultLocale": {"name": "Global"}}
        ]
    });
    write(&profile.join("extensions.json"), &db.to_string());
    let startup = json!({
        "app-profile": {"path": profile.join("extensions").to_string_lossy(), "addons": {
            FX_UBO: {"enabled": true, "lastModifiedTime": 1700000000000u64, "path": "x", "version": "1.60.0", "type": "extension", "dependencies": [], "hasEmbeddedWebExtension": false},
            FX_THEME: {"enabled": false, "type": "theme"}
        }},
        "app-system-defaults": {"addons": {}}
    });
    fs::write(
        profile.join("addonStartup.json.lz4"),
        mozlz4::compress(startup.to_string().as_bytes()),
    )
    .unwrap();
    write(
        &profile.join("extensions").join(format!("{FX_UBO}.xpi")),
        "PK-zip-bytes",
    );
    Fox { profile }
}

fn startup_json(f: &Fox) -> Value {
    let raw =
        mozlz4::decompress(&fs::read(f.profile.join("addonStartup.json.lz4")).unwrap()).unwrap();
    serde_json::from_slice(&raw).unwrap()
}

fn fid(ext: &str) -> String {
    format!("firefox:abcd1234.default-release:{ext}")
}

#[test]
fn firefox_listing_skips_builtin_and_hidden_addons() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    firefox_fixture(&c);
    let v = call(&c, "browser_plugins.list", json!({})).unwrap();
    let ids: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["extensionId"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 3, "{ids:?}");
    let u = plugin(&v, FX_UBO);
    assert_eq!(u["name"], "uBlock Origin");
    assert_eq!(u["version"], "1.60.0");
    assert_eq!(u["type"], "extension");
    assert_eq!(u["enabled"], true);
    assert_eq!(u["canDisable"], true);
    assert_eq!(u["canRemove"], true);
    assert_eq!(u["browser"], "firefox");
    let t = plugin(&v, FX_THEME);
    assert_eq!(t["type"], "theme");
    assert_eq!(t["enabled"], false);
    assert_eq!(t["canRemove"], false, "no file in the profile to remove");
    let g = plugin(&v, "global@example.org");
    assert_eq!(g["canRemove"], false);
    assert!(g["note"].as_str().is_some());
}

#[test]
fn firefox_disable_updates_both_files_and_enable_restores_them() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let fx = firefox_fixture(&c);
    let db_before = read_json(&fx.profile.join("extensions.json"));
    let startup_before = startup_json(&fx);
    let r = call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": fid(FX_UBO), "enabled": false}),
    )
    .unwrap();
    assert_eq!(r["plugin"]["enabled"], false);
    let db = read_json(&fx.profile.join("extensions.json"));
    assert_eq!(db["addons"][0]["userDisabled"], true);
    assert_eq!(db["addons"][0]["active"], false);
    // everything else is intact
    let mut expected = db_before.clone();
    expected["addons"][0]["userDisabled"] = json!(true);
    expected["addons"][0]["active"] = json!(false);
    assert_eq!(db, expected);
    let st = startup_json(&fx);
    assert_eq!(st["app-profile"]["addons"][FX_UBO]["enabled"], false);
    assert_eq!(
        st["app-profile"]["addons"][FX_UBO]["lastModifiedTime"],
        1700000000000u64
    );
    assert_eq!(
        st["app-system-defaults"],
        startup_before["app-system-defaults"]
    );
    // the file really is mozLz4
    let raw = fs::read(fx.profile.join("addonStartup.json.lz4")).unwrap();
    assert_eq!(&raw[..8], b"mozLz40\0");

    // both files were backed up first
    let dir = c
        .env
        .data_dir
        .join("backups")
        .join(r["backupId"].as_str().unwrap());
    let m = read_json(&dir.join("manifest.json"));
    assert_eq!(m["items"].as_array().unwrap().len(), 2);
    let ej = m["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["original"].as_str().unwrap().ends_with("extensions.json"))
        .unwrap();
    assert_eq!(
        read_json(&dir.join(ej["backup"].as_str().unwrap())),
        db_before
    );

    call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": fid(FX_UBO), "enabled": true}),
    )
    .unwrap();
    assert_eq!(read_json(&fx.profile.join("extensions.json")), db_before);
    assert_eq!(startup_json(&fx), startup_before);
}

#[test]
fn firefox_damaged_startup_cache_aborts_before_writing_anything() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let fx = firefox_fixture(&c);
    fs::write(fx.profile.join("addonStartup.json.lz4"), b"garbage-not-lz4").unwrap();
    let before = fs::read(fx.profile.join("extensions.json")).unwrap();
    assert!(call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": fid(FX_UBO), "enabled": false})
    )
    .is_err());
    assert_eq!(
        fs::read(fx.profile.join("extensions.json")).unwrap(),
        before
    );
}

#[test]
fn firefox_without_a_startup_cache_only_edits_extensions_json() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let fx = firefox_fixture(&c);
    fs::remove_file(fx.profile.join("addonStartup.json.lz4")).unwrap();
    call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": fid(FX_UBO), "enabled": false}),
    )
    .unwrap();
    assert_eq!(
        read_json(&fx.profile.join("extensions.json"))["addons"][0]["userDisabled"],
        true
    );
    assert!(!fx.profile.join("addonStartup.json.lz4").exists());
}

#[test]
fn firefox_running_blocks_and_remove_backs_up_the_xpi() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &["firefox"]);
    let fx = firefox_fixture(&c);
    let e = call(&c, "browser_plugins.remove", json!({"id": fid(FX_UBO)})).unwrap_err();
    assert!(
        e.message.starts_with("Close Firefox first"),
        "{}",
        e.message
    );
    assert!(fx
        .profile
        .join("extensions")
        .join(format!("{FX_UBO}.xpi"))
        .exists());

    let c = ctx(d.path(), &[]);
    let r = call(&c, "browser_plugins.remove", json!({"id": fid(FX_UBO)})).unwrap();
    let xpi = fx.profile.join("extensions").join(format!("{FX_UBO}.xpi"));
    assert!(!xpi.exists());
    let db = read_json(&fx.profile.join("extensions.json"));
    assert_eq!(db["addons"][0]["active"], false);
    assert_eq!(
        startup_json(&fx)["app-profile"]["addons"][FX_UBO]["enabled"],
        false
    );
    let dir = c
        .env
        .data_dir
        .join("backups")
        .join(r["backupId"].as_str().unwrap());
    let m = read_json(&dir.join("manifest.json"));
    let x = m["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["original"].as_str().unwrap().ends_with(".xpi"))
        .unwrap();
    assert_eq!(
        fs::read_to_string(dir.join(x["backup"].as_str().unwrap())).unwrap(),
        "PK-zip-bytes"
    );
    assert!(r["note"].as_str().unwrap().contains("next time"));
}

#[test]
fn firefox_unpacked_addon_directory_is_removed_through_safe_deleter() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let fx = firefox_fixture(&c);
    fs::remove_file(fx.profile.join("extensions").join(format!("{FX_UBO}.xpi"))).unwrap();
    write(
        &fx.profile
            .join("extensions")
            .join(FX_UBO)
            .join("manifest.json"),
        "{}",
    );
    write(
        &fx.profile
            .join("extensions")
            .join(FX_UBO)
            .join("sub/deep.txt"),
        "x",
    );
    call(&c, "browser_plugins.remove", json!({"id": fid(FX_UBO)})).unwrap();
    assert!(!fx.profile.join("extensions").join(FX_UBO).exists());
    assert!(fx.profile.join("extensions").exists());
}

#[test]
fn remove_tree_refuses_paths_outside_its_base() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let base = d.path().join("base");
    fs::create_dir_all(&base).unwrap();
    let outside = d.path().join("outside");
    write(&outside.join("a.txt"), "a");
    assert!(remove_tree(&c, &base, &outside).is_err());
    assert!(outside.join("a.txt").exists());
    // the base itself is refused too
    assert!(remove_tree(&c, &base, &base).is_err());
}

// ---------------------------------------------------------------- real Chromium (ignored)

/// Builds a REAL Chromium profile with an unpacked extension by running the pre-installed
/// Chromium headlessly, then lists it. Needs Chromium under `/opt/pw-browsers` (or
/// `$CLEARSWEEP_CHROMIUM`) and skips itself otherwise.
///
/// Observed with Chromium 141.0.7390.37 (Playwright build 1194) on Linux, launched as
/// `--headless=new --no-sandbox --user-data-dir=<tmp>/.config/chromium --load-extension=<dir>`:
/// - the extension settings live in the plain `Preferences` file (`extensions.settings.<id>`);
///   `Secure Preferences` only holds `protection.super_mac`;
/// - the command-line extension is `location: 8` with an absolute `path` and no embedded
///   `manifest`, and only the new `disable_reasons: []` format (no `state` key) is used;
/// - EVERY entry, including this one, has an HMAC record in `Preferences` under
///   `protection.macs.extensions.settings.<id>` (plus `settings_encrypted_hash`), so editing
///   the entry would break the MAC. The lister therefore reports `canDisable: false` with the
///   protection note even though the file is not "Secure Preferences".
#[test]
#[ignore = "runs a real Chromium; needs /opt/pw-browsers (run by scripts/check.sh as root)"]
fn real_chromium_profile_with_an_unpacked_extension() {
    let chrome = std::env::var_os("CLEARSWEEP_CHROMIUM")
        .map(PathBuf::from)
        .or_else(|| {
            let base = Path::new("/opt/pw-browsers");
            fs::read_dir(base).ok()?.flatten().find_map(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                let p = e.path().join("chrome-linux/chrome");
                (n.starts_with("chromium-") && p.is_file()).then_some(p)
            })
        });
    let Some(chrome) = chrome else {
        eprintln!("skipping: no Chromium found");
        return;
    };
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    let ext = d.path().join("ext");
    write(
        &ext.join("manifest.json"),
        r#"{"manifest_version":3,"name":"ClearSweep Real Test","version":"4.5.6","description":"Real profile check"}"#,
    );
    // Chromium reads its user-data dir from HOME/.config/chromium when given no flag; we
    // pass the same path explicitly so the profile lands where the plugin lister looks.
    let user_data = home.join(".config/chromium");
    fs::create_dir_all(&user_data).unwrap();
    let mut child = std::process::Command::new(&chrome)
        .args([
            "--headless=new",
            "--no-sandbox",
            "--disable-gpu",
            "--no-first-run",
            "--disable-dev-shm-usage",
        ])
        .arg(format!("--user-data-dir={}", user_data.display()))
        .arg(format!("--load-extension={}", ext.display()))
        .arg("about:blank")
        .env("HOME", &home)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start chromium");
    // Wait until the profile has been written (extension registered), then stop it
    // gracefully so preferences are flushed.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
    let prefs = user_data.join("Default/Preferences");
    let secure = user_data.join("Default/Secure Preferences");
    let mut seen = false;
    while std::time::Instant::now() < deadline {
        let hit =
            |p: &Path| fs::read_to_string(p).is_ok_and(|t| t.contains(&*ext.to_string_lossy()));
        {
            if hit(&prefs) || hit(&secure) {
                seen = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let _ = child.wait();
    assert!(
        seen,
        "Chromium never registered the extension in Preferences / Secure Preferences"
    );

    let mut c = Ctx::new(Env::for_test(d.path()), Arc::new(MockRunner::new()))
        .with_procs(Arc::new(FakeProcesses::new(&[], true)));
    c.env.home = home.clone();
    c.env.config_dir = home.join(".config");
    c.env.os = Os::Linux;
    let v = call(&c, "browser_plugins.list", json!({})).unwrap();
    let p = v
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "ClearSweep Real Test")
        .unwrap_or_else(|| panic!("extension not listed: {v}"));
    assert_eq!(p["version"], "4.5.6");
    assert_eq!(p["browser"], "chromium");
    let p_json = read_json(&prefs);
    let id = p["extensionId"].as_str().unwrap();
    let in_prefs = p_json["extensions"]["settings"].get(id).is_some();
    let has_mac = p_json["protection"]["macs"]["extensions"]["settings"]
        .get(id)
        .is_some();
    let in_secure = read_json(&secure)["extensions"]["settings"]
        .get(id)
        .is_some();
    eprintln!(
        "real Chromium: entry in Preferences={in_prefs}, in Secure Preferences={in_secure}, MAC record in Preferences={has_mac}; location={}",
        p_json["extensions"]["settings"][id]["location"]
    );
    assert!(in_prefs || in_secure);
    // The lister must agree with the files: a MAC-protected entry is never editable.
    assert_eq!(p["canDisable"], !(in_secure || has_mac), "{p}");
    if in_secure || has_mac {
        assert!(p["note"]
            .as_str()
            .unwrap()
            .contains("protects extension settings"));
    }
    assert_eq!(p["enabled"], true);
}

// ---------------------------------------------------------------- restore from a backup

fn backup_dir(c: &Ctx, id: &str) -> PathBuf {
    c.env.data_dir.join("backups").join(id)
}

fn edit_manifest(c: &Ctx, id: &str, f: impl FnOnce(&mut Value)) {
    let p = backup_dir(c, id).join("manifest.json");
    let mut m = read_json(&p);
    f(&mut m);
    fs::write(&p, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
}

fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            (
                e.path()
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

fn remove_a(c: &Ctx) -> String {
    call(c, "browser_plugins.remove", json!({"id": pid(ID_A)})).unwrap()["backupId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn remove_ubo(c: &Ctx) -> String {
    call(c, "browser_plugins.remove", json!({"id": fid(FX_UBO)})).unwrap()["backupId"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn chromium_restore_round_trip_through_the_restore_feature() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let prefs_before = read_json(&ch.prefs());
    let dir_before = tree(&ch.ext(ID_A));
    let bid = remove_a(&c);
    assert!(!ch.ext(ID_A).exists());

    // System Restore lists it with a clear description and can restore it.
    let list = call(&c, "restore.list_points", json!({})).unwrap();
    let pt = list["points"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == format!("clearsweep:{bid}"))
        .unwrap_or_else(|| panic!("backup not listed: {list}"));
    assert_eq!(
        pt["description"],
        "Removed browser add-on: Ad Blocker Pro (Google Chrome)"
    );
    assert_eq!(pt["backupKind"], "plugins");
    assert_eq!(pt["restorable"], true);

    let r = call(
        &c,
        "restore.restore",
        json!({"id": format!("clearsweep:{bid}")}),
    )
    .unwrap();
    assert_eq!(r["ok"], true);
    assert!(r["message"]
        .as_str()
        .unwrap()
        .starts_with("Restored browser add-on Ad Blocker Pro (Google Chrome)"));
    assert_eq!(tree(&ch.ext(ID_A)), dir_before);
    assert_eq!(read_json(&ch.prefs()), prefs_before);
    let listing = call(&c, "browser_plugins.list", json!({})).unwrap();
    assert_eq!(plugin(&listing, ID_A)["name"], "Ad Blocker Pro");

    // The direct method does the same (after removing it once more).
    let bid2 = remove_a(&c);
    let r = call(&c, "browser_plugins.restore_backup", json!({"id": bid2})).unwrap();
    assert_eq!(r["ok"], true);
    assert_eq!(tree(&ch.ext(ID_A)), dir_before);
    assert_eq!(read_json(&ch.prefs()), prefs_before);
}

#[test]
fn chromium_restore_keeps_changes_made_to_preferences_since() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let bid = remove_a(&c);
    // Something else changed while A was gone: B was disabled-by-edit and a new key appeared.
    let mut p = read_json(&ch.prefs());
    p["browser"]["new_key"] = json!("kept");
    p["extensions"]["settings"][ID_B]["state"] = json!(1);
    fs::write(ch.prefs(), p.to_string()).unwrap();
    call(&c, "browser_plugins.restore_backup", json!({"id": bid})).unwrap();
    let after = read_json(&ch.prefs());
    assert_eq!(after["browser"]["new_key"], "kept");
    assert_eq!(after["extensions"]["settings"][ID_B]["state"], 1);
    assert_eq!(after["extensions"]["settings"][ID_A]["state"], 1);
    assert_eq!(after["extensions"]["settings"][ID_A]["from_webstore"], true);
}

#[test]
fn chromium_restore_puts_the_mac_record_back_too() {
    let d = tempfile::tempdir().unwrap();
    let files = chromium::ProfileFiles {
        dir: d.path().join("Default"),
    };
    let saved = json!({
        "extensions": {"settings": {ID_A: {"state": 1}}},
        "protection": {"macs": {"extensions": {"settings": {ID_A: "MAC"}}}}
    });
    write(&files.prefs(), r#"{"keep": 1}"#);
    assert!(chromium::restore_pref_entry(&files, ID_A, &saved).unwrap());
    let after = read_json(&files.prefs());
    assert_eq!(after["keep"], 1);
    assert_eq!(after["extensions"]["settings"][ID_A]["state"], 1);
    assert_eq!(
        after["protection"]["macs"]["extensions"]["settings"][ID_A],
        "MAC"
    );
    // Nothing saved for the id: nothing written.
    assert!(!chromium::restore_pref_entry(&files, ID_B, &saved).unwrap());
}

#[test]
fn chromium_secure_entry_removal_restores_only_the_folder_and_says_so() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let r = call(&c, "browser_plugins.remove", json!({"id": pid(ID_C)})).unwrap();
    let bid = r["backupId"].as_str().unwrap();
    assert!(!ch.ext(ID_C).exists());
    let secure = fs::read(ch.secure()).unwrap();
    let prefs = fs::read(ch.prefs()).unwrap();
    let out = call(&c, "browser_plugins.restore_backup", json!({"id": bid})).unwrap();
    assert!(ch.ext(ID_C).join("3.0_0/manifest.json").is_file());
    assert_eq!(fs::read(ch.prefs()).unwrap(), prefs);
    assert_eq!(fs::read(ch.secure()).unwrap(), secure);
    assert!(!out["notes"].as_array().unwrap().is_empty());
}

#[test]
fn firefox_restore_round_trip_xpi_extensions_json_and_startup_cache() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let fx = firefox_fixture(&c);
    let db_before = read_json(&fx.profile.join("extensions.json"));
    let startup_before = startup_json(&fx);
    let xpi = fx.profile.join("extensions").join(format!("{FX_UBO}.xpi"));
    let bid = remove_ubo(&c);
    assert!(!xpi.exists());
    assert_ne!(read_json(&fx.profile.join("extensions.json")), db_before);

    let list = call(&c, "restore.list_points", json!({})).unwrap();
    let pt = list["points"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == format!("clearsweep:{bid}"))
        .unwrap();
    assert_eq!(
        pt["description"],
        "Removed browser add-on: uBlock Origin (Firefox)"
    );
    assert_eq!(pt["restorable"], true);

    call(
        &c,
        "restore.restore",
        json!({"id": format!("clearsweep:{bid}")}),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&xpi).unwrap(), "PK-zip-bytes");
    assert_eq!(read_json(&fx.profile.join("extensions.json")), db_before);
    assert_eq!(startup_json(&fx), startup_before);
    let raw = fs::read(fx.profile.join("addonStartup.json.lz4")).unwrap();
    assert_eq!(&raw[..8], b"mozLz40\0");
    let listing = call(&c, "browser_plugins.list", json!({})).unwrap();
    let u = plugin(&listing, FX_UBO);
    assert_eq!(u["enabled"], true);
    assert_eq!(u["canRemove"], true);
}

#[test]
fn firefox_restore_adds_the_record_when_firefox_already_dropped_it() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let fx = firefox_fixture(&c);
    let bid = remove_ubo(&c);
    // Firefox started in between: it forgot the add-on and its startup entry.
    let ej = fx.profile.join("extensions.json");
    let mut db = read_json(&ej);
    db["addons"]
        .as_array_mut()
        .unwrap()
        .retain(|a| a["id"] != FX_UBO);
    db["schemaVersion"] = json!(37);
    fs::write(&ej, db.to_string()).unwrap();
    let mut st = startup_json(&fx);
    st["app-profile"]["addons"]
        .as_object_mut()
        .unwrap()
        .remove(FX_UBO);
    fs::write(
        fx.profile.join("addonStartup.json.lz4"),
        mozlz4::compress(st.to_string().as_bytes()),
    )
    .unwrap();
    call(&c, "browser_plugins.restore_backup", json!({"id": bid})).unwrap();
    let db = read_json(&ej);
    assert_eq!(db["schemaVersion"], 37, "other keys are kept");
    let back = db["addons"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == FX_UBO)
        .unwrap();
    assert_eq!(back["active"], true);
    assert_eq!(back["unknownKey"], json!({"keep": [1, 2, 3]}));
    assert_eq!(
        startup_json(&fx)["app-profile"]["addons"][FX_UBO]["enabled"],
        true
    );
}

#[test]
fn restore_refuses_while_the_browser_is_running_and_changes_nothing() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let fx = firefox_fixture(&c);
    let a = remove_a(&c);
    let u = remove_ubo(&c);
    let prefs = fs::read(ch.prefs()).unwrap();
    let ejson = fs::read(fx.profile.join("extensions.json")).unwrap();
    let busy = ctx(d.path(), &["chrome", "firefox"]);
    for bid in [&a, &u] {
        let e = call(&busy, "browser_plugins.restore_backup", json!({"id": bid})).unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied);
        assert!(e.message.contains("Close"), "{}", e.message);
        let e = call(
            &busy,
            "restore.restore",
            json!({"id": format!("clearsweep:{bid}")}),
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied);
    }
    assert!(!ch.ext(ID_A).exists());
    assert!(!fx
        .profile
        .join("extensions")
        .join(format!("{FX_UBO}.xpi"))
        .exists());
    assert_eq!(fs::read(ch.prefs()).unwrap(), prefs);
    assert_eq!(fs::read(fx.profile.join("extensions.json")).unwrap(), ejson);
}

#[test]
fn restore_never_overwrites_an_existing_extension() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let fx = firefox_fixture(&c);
    let a = remove_a(&c);
    let u = remove_ubo(&c);
    // The user reinstalled newer versions in the meantime.
    write(
        &ch.ext(ID_A).join("9.9_0/manifest.json"),
        r#"{"name":"Newer","version":"9.9"}"#,
    );
    let xpi = fx.profile.join("extensions").join(format!("{FX_UBO}.xpi"));
    write(&xpi, "newer xpi");
    let prefs = fs::read(ch.prefs()).unwrap();
    let ejson = fs::read(fx.profile.join("extensions.json")).unwrap();
    for bid in [&a, &u] {
        let e = call(&c, "browser_plugins.restore_backup", json!({"id": bid})).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams);
        assert!(e.message.contains("already has"), "{}", e.message);
        assert!(e.message.contains("not replaced"), "{}", e.message);
    }
    assert_eq!(fs::read_to_string(&xpi).unwrap(), "newer xpi");
    assert!(ch.ext(ID_A).join("9.9_0/manifest.json").is_file());
    assert!(!ch.ext(ID_A).join("1.2.3_0").exists());
    assert_eq!(fs::read(ch.prefs()).unwrap(), prefs);
    assert_eq!(fs::read(fx.profile.join("extensions.json")).unwrap(), ejson);
}

#[test]
fn restore_refuses_manifest_paths_outside_the_recorded_profile() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let outside = d.path().join("outside");
    fs::create_dir_all(&outside).unwrap();

    // 1. The payload's recorded target points somewhere else.
    let bid = remove_a(&c);
    let evil = outside.join(ID_A);
    edit_manifest(&c, &bid, |m| {
        m["items"][0]["original"] = json!(evil.to_string_lossy());
    });
    let e = call(&c, "browser_plugins.restore_backup", json!({"id": bid})).unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(!evil.exists());
    assert!(!ch.ext(ID_A).exists());

    // 2. A `..` escape that looks like it starts inside the profile.
    let bid = remove_a_again(&c, &ch);
    let sneaky = ch
        .profile
        .join("Extensions")
        .join("..")
        .join("..")
        .join("escaped");
    edit_manifest(&c, &bid, |m| {
        m["items"][0]["original"] = json!(sneaky.to_string_lossy());
    });
    let e = call(&c, "browser_plugins.restore_backup", json!({"id": bid})).unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(!d.path().join("escaped").exists());

    // 3. The Preferences item aimed at another file.
    let bid = remove_a_again(&c, &ch);
    let target = outside.join("Preferences");
    edit_manifest(&c, &bid, |m| {
        let items = m["items"].as_array_mut().unwrap();
        let pref = items.iter_mut().find(|i| i["role"] == "prefs").unwrap();
        pref["original"] = json!(target.to_string_lossy());
    });
    let e = call(&c, "browser_plugins.restore_backup", json!({"id": bid})).unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(!target.exists());
    assert!(!ch.ext(ID_A).exists());

    // 4. The recorded profile is not one of the machine's profiles.
    let bid = remove_a_again(&c, &ch);
    edit_manifest(&c, &bid, |m| {
        m["plugin"]["profileDir"] = json!(outside.to_string_lossy());
    });
    let e = call(&c, "browser_plugins.restore_backup", json!({"id": bid})).unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    assert!(fs::read_dir(&outside).unwrap().next().is_none());

    // 5. The saved copy is outside the backup folder.
    let bid = remove_a_again(&c, &ch);
    edit_manifest(&c, &bid, |m| {
        m["items"][0]["backup"] = json!("../../../etc/passwd");
    });
    let e = call(&c, "browser_plugins.restore_backup", json!({"id": bid})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Io);
    assert!(!ch.ext(ID_A).exists());

    // 6. An item with a role this browser does not use.
    let bid = remove_a_again(&c, &ch);
    edit_manifest(&c, &bid, |m| {
        m["items"][0]["role"] = json!("extensions-json");
    });
    let e = call(&c, "browser_plugins.restore_backup", json!({"id": bid})).unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
}

/// The fixture's A was restored or is gone: make sure it is installed, then remove it again.
fn remove_a_again(c: &Ctx, ch: &Chrome) -> String {
    if !ch.ext(ID_A).exists() {
        manifest(
            &ch.ext(ID_A).join("1.2.3_0"),
            r#"{"name":"Ad Blocker Pro","version":"1.2.3"}"#,
        );
        let mut p = read_json(&ch.prefs());
        p["extensions"]["settings"][ID_A] =
            json!({"location": 1, "path": format!("{ID_A}/1.2.3_0"), "state": 1});
        fs::write(ch.prefs(), p.to_string()).unwrap();
    }
    remove_a(c)
}

#[test]
fn restore_rejects_bad_ids_other_kinds_and_backups_that_are_not_removals() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    for bad in [
        "plugins-../x",
        "plugins-1/2",
        "startup-1700000000",
        "../plugins-1",
        "",
    ] {
        let e = call(&c, "browser_plugins.restore_backup", json!({"id": bad})).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams, "{bad}");
    }
    let e = call(
        &c,
        "browser_plugins.restore_backup",
        json!({"id": "plugins-1"}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);

    // A plain disable is a backup too, but there is nothing to put back.
    let r = call(
        &c,
        "browser_plugins.set_enabled",
        json!({"id": pid(ID_A), "enabled": false}),
    )
    .unwrap();
    let bid = r["backupId"].as_str().unwrap();
    let e = call(&c, "browser_plugins.restore_backup", json!({"id": bid})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    assert!(ch.ext(ID_A).exists());
    let list = call(&c, "restore.list_points", json!({})).unwrap();
    assert!(
        list["points"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["id"] != format!("clearsweep:{bid}")),
        "an enable/disable backup is not offered in System Restore"
    );
}

#[test]
fn backups_without_the_profile_record_are_listed_but_not_restorable() {
    let d = tempfile::tempdir().unwrap();
    let c = ctx(d.path(), &[]);
    let ch = chrome_fixture(&c);
    let bid = remove_a(&c);
    edit_manifest(&c, &bid, |m| {
        m.as_object_mut().unwrap().remove("plugin");
        m["description"] = json!("Removed Ad Blocker Pro from Google Chrome");
    });
    let list = call(&c, "restore.list_points", json!({})).unwrap();
    let pt = list["points"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == format!("clearsweep:{bid}"))
        .unwrap();
    assert_eq!(pt["restorable"], false);
    let e = call(&c, "browser_plugins.restore_backup", json!({"id": bid})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    assert!(!ch.ext(ID_A).exists());
}
