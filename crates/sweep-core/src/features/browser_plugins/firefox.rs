//! Firefox add-ons: `extensions.json` (the add-on database) and `addonStartup.json.lz4`
//! (the startup cache, mozLz4). Disabling changes both, because Firefox trusts the startup
//! cache at launch; both are written atomically after being backed up.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{ApiError, Result};
use crate::fsutil::atomic_write;

use super::chromium::valid_ext_id;
use super::{mozlz4, Plugin};

pub const NOTE_SYSTEM: &str = "Installed with Firefox or by the system; it cannot be removed here";

pub fn extensions_json(profile: &Path) -> PathBuf {
    profile.join("extensions.json")
}
pub fn startup_lz4(profile: &Path) -> PathBuf {
    profile.join("addonStartup.json.lz4")
}

fn read_json(p: &Path) -> Option<Value> {
    let b = fs::read(p).ok()?;
    serde_json::from_str(String::from_utf8_lossy(&b).trim_start_matches('\u{feff}')).ok()
}

fn map_type(t: &str) -> Option<&'static str> {
    match t {
        "extension" => Some("extension"),
        "theme" => Some("theme"),
        "locale" => Some("locale"),
        "dictionary" | "webextension-dictionary" => Some("dictionary"),
        "plugin" => Some("plugin"),
        _ => None,
    }
}

/// Add-ons that ship with Firefox itself are not listed.
fn is_builtin_location(loc: &str) -> bool {
    loc.starts_with("app-builtin") || loc == "app-system-defaults"
}

pub fn list_profile(
    browser: &'static str,
    label: &str,
    profile: &Path,
    profile_label: &str,
    running: bool,
) -> Vec<Plugin> {
    let Some(db) = read_json(&extensions_json(profile)) else {
        return Vec::new();
    };
    let Some(addons) = db.get("addons").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for a in addons {
        let Some(id) = a.get("id").and_then(Value::as_str) else {
            continue;
        };
        let location = a.get("location").and_then(Value::as_str).unwrap_or("");
        if is_builtin_location(location) || !valid_ext_id(id) {
            continue;
        }
        if a.get("visible").and_then(Value::as_bool) == Some(false)
            || a.get("hidden").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(kind) = a.get("type").and_then(Value::as_str).and_then(map_type) else {
            continue;
        };
        let loc = a.get("defaultLocale");
        let name = loc
            .and_then(|l| l.get("name"))
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string();
        let description = loc
            .and_then(|l| l.get("description"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let user_disabled = a
            .get("userDisabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let app_disabled = a
            .get("appDisabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let profile_addon = location == "app-profile";
        let mut note = None;
        if app_disabled && !user_disabled {
            note = Some("Firefox turned this off (incompatible or unverified)".to_string());
        } else if !profile_addon {
            note = Some(NOTE_SYSTEM.to_string());
        }
        let install = a
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| profile.join("extensions").to_string_lossy().into_owned());
        out.push(Plugin {
            id: format!("{browser}:{profile_label}:{id}"),
            browser: browser.to_string(),
            browser_label: label.to_string(),
            profile: profile_label.to_string(),
            profile_name: None,
            extension_id: id.to_string(),
            name,
            version: a
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            description,
            enabled: !user_disabled && !app_disabled,
            kind: kind.to_string(),
            install_location: install,
            can_disable: true,
            can_remove: profile_addon && addon_payload(profile, id).is_some(),
            note,
            running,
        });
    }
    out
}

/// `extensions.json` with the add-on's state changed. Every other key is kept.
pub fn edit_extensions_json(db: &mut Value, id: &str, f: impl FnOnce(&mut Value)) -> Result<()> {
    let addon = db
        .get_mut("addons")
        .and_then(Value::as_array_mut)
        .and_then(|a| {
            a.iter_mut()
                .find(|x| x.get("id").and_then(Value::as_str) == Some(id))
        })
        .ok_or_else(|| ApiError::not_found("the add-on is not in extensions.json"))?;
    f(addon);
    Ok(())
}

pub fn set_flags(addon: &mut Value, enabled: bool) {
    if let Some(o) = addon.as_object_mut() {
        o.insert("userDisabled".into(), Value::Bool(!enabled));
        o.insert("active".into(), Value::Bool(enabled));
    }
}

pub fn set_inactive(addon: &mut Value) {
    if let Some(o) = addon.as_object_mut() {
        o.insert("active".into(), Value::Bool(false));
    }
}

/// Set `enabled` for `id` in every location of a decoded `addonStartup.json`.
/// Returns whether the add-on was found.
pub fn set_startup_enabled(startup: &mut Value, id: &str, enabled: bool) -> bool {
    let mut found = false;
    if let Some(locs) = startup.as_object_mut() {
        for (_, loc) in locs.iter_mut() {
            if let Some(a) = loc
                .get_mut("addons")
                .and_then(Value::as_object_mut)
                .and_then(|m| m.get_mut(id))
                .and_then(Value::as_object_mut)
            {
                a.insert("enabled".into(), Value::Bool(enabled));
                found = true;
            }
        }
    }
    found
}

/// The new file contents for both files, computed before anything is written.
pub struct Edited {
    pub json: Vec<u8>,
    pub startup: Option<Vec<u8>>,
}

pub fn compute_edit(
    profile: &Path,
    id: &str,
    f: impl FnOnce(&mut Value),
    startup_enabled: bool,
) -> Result<Edited> {
    let path = extensions_json(profile);
    let mut db = read_json(&path)
        .ok_or_else(|| ApiError::io(format!("could not read {}", path.display())))?;
    edit_extensions_json(&mut db, id, f)?;
    let json = serde_json::to_vec(&db)?;
    let startup = match fs::read(startup_lz4(profile)) {
        Ok(bytes) => {
            let raw = mozlz4::decompress(&bytes)?;
            let mut v: Value = serde_json::from_slice(&raw)
                .map_err(|e| ApiError::io(format!("addonStartup.json.lz4 is damaged: {e}")))?;
            if set_startup_enabled(&mut v, id, startup_enabled) {
                Some(mozlz4::compress(&serde_json::to_vec(&v)?))
            } else {
                None
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    Ok(Edited { json, startup })
}

/// Write the edit: `extensions.json` first, then the startup cache; if the second write
/// fails the first is rolled back from `original_json`.
pub fn write_edit(profile: &Path, edit: &Edited, original_json: &[u8]) -> Result<()> {
    atomic_write(&extensions_json(profile), &edit.json)
        .map_err(|e| ApiError::io(format!("could not write extensions.json: {e}")))?;
    if let Some(s) = &edit.startup {
        if let Err(e) = atomic_write(&startup_lz4(profile), s) {
            let _ = atomic_write(&extensions_json(profile), original_json);
            return Err(ApiError::io(format!(
                "could not write addonStartup.json.lz4: {e}"
            )));
        }
    }
    Ok(())
}

/// The file or folder holding a profile add-on: `<profile>/extensions/<id>.xpi` or
/// `<profile>/extensions/<id>/`.
pub fn addon_payload(profile: &Path, id: &str) -> Option<PathBuf> {
    if !valid_ext_id(id) {
        return None;
    }
    let dir = profile.join("extensions");
    let xpi = dir.join(format!("{id}.xpi"));
    if xpi.is_file() {
        return Some(xpi);
    }
    let unpacked = dir.join(id);
    unpacked.is_dir().then_some(unpacked)
}
