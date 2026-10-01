//! Putting a removed browser add-on back from its `plugins-<ts>` backup.
//!
//! `browser_plugins.restore_backup { id }` (also reached through `restore.restore`). The backup
//! manifest records which browser profile the add-on came from (`plugin`) and every saved file
//! has a `role`. Nothing in the manifest is trusted for a path: the profile must still be one
//! this machine's browser discovery finds, and each saved file's recorded original path must
//! equal the path that role has inside that profile. The browser must be closed, and an
//! existing extension folder / `.xpi` is never overwritten.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use super::{
    chromium, discover, firefox, is_running, remove_tree, running_error, Family, Plugin,
    ProfileRef, BROWSERS,
};
use crate::ctx::Ctx;
use crate::error::{ApiError, Result};
use crate::features::startup::backup::{copy_dir, read_manifest};
use crate::job::Job;

pub const ROLE_PAYLOAD: &str = "payload";
pub const ROLE_PREFS: &str = "prefs";
pub const ROLE_EXTENSIONS_JSON: &str = "extensions-json";
pub const ROLE_STARTUP: &str = "startup-cache";

/// The manifest's `plugin` record, written with every `plugins` backup.
pub fn meta(profile: &ProfileRef, plugin: &Plugin, action: &str) -> Value {
    json!({
        "action": action,
        "browser": profile.def.key,
        "browserLabel": profile.def.label,
        "family": match profile.def.family {
            Family::Chromium => "chromium",
            Family::Firefox => "firefox",
        },
        "profile": profile.label,
        "profileDir": profile.dir.to_string_lossy(),
        "extensionId": plugin.extension_id,
        "name": plugin.name,
    })
}

/// Should System Restore list this backup? Only removals are restorable, so backups of a
/// plain enable/disable are not shown. Backups made before the record existed are shown
/// (by their description) but cannot be restored.
pub fn listed(manifest: &Value) -> bool {
    match manifest
        .get("plugin")
        .and_then(|p| p.get("action"))
        .and_then(Value::as_str)
    {
        Some(a) => a == "remove",
        None => manifest
            .get("description")
            .and_then(Value::as_str)
            .is_some_and(|d| d.starts_with("Removed")),
    }
}

/// Can ClearSweep restore this (listed) backup?
pub fn restorable(manifest: &Value) -> bool {
    Meta::parse(manifest).is_ok()
}

struct Meta {
    browser: String,
    family: Family,
    profile_dir: PathBuf,
    ext: String,
    name: String,
}

impl Meta {
    fn parse(manifest: &Value) -> Result<Meta> {
        let m = manifest.get("plugin").ok_or_else(|| {
            ApiError::unsupported(
                "This backup was made by an older version that did not record where the add-on came from, so it cannot be restored automatically.",
            )
        })?;
        let s = |k: &str| {
            m.get(k)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .ok_or_else(|| ApiError::io(format!("damaged backup: no `{k}`")))
        };
        if s("action")? != "remove" {
            return Err(ApiError::unsupported(
                "This backup is of a setting change, not of a removed add-on.",
            ));
        }
        let family = match s("family")?.as_str() {
            "chromium" => Family::Chromium,
            "firefox" => Family::Firefox,
            _ => return Err(ApiError::io("damaged backup: unknown browser family")),
        };
        let ext = s("extensionId")?;
        if !chromium::valid_ext_id(&ext) {
            return Err(ApiError::io("damaged backup: bad add-on id"));
        }
        Ok(Meta {
            browser: s("browser")?,
            family,
            profile_dir: PathBuf::from(s("profileDir")?),
            ext,
            name: s("name")?,
        })
    }
}

/// One saved file, after its paths have been checked.
struct Saved {
    is_dir: bool,
    /// Where it belongs in the profile.
    target: PathBuf,
    /// The copy inside the backup folder.
    source: PathBuf,
}

/// The saved items by role. Every item's recorded original must be exactly the path its role
/// has inside `profile`, and its copy must be a plain file/folder inside the backup folder.
fn saved_items(
    backup_dir: &Path,
    manifest: &Value,
    meta: &Meta,
    profile: &Path,
) -> Result<Vec<(String, Saved)>> {
    let items = manifest
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::io("damaged backup: no items"))?;
    let mut out: Vec<(String, Saved)> = Vec::new();
    for it in items {
        let s = |k: &str| it.get(k).and_then(Value::as_str);
        let role = s("role").ok_or_else(|| ApiError::io("damaged backup: an item has no role"))?;
        let ty = s("type").unwrap_or("");
        let original = PathBuf::from(
            s("original").ok_or_else(|| ApiError::io("damaged backup: an item has no path"))?,
        );
        let expected: Vec<(PathBuf, bool)> = match (meta.family, role) {
            (Family::Chromium, ROLE_PAYLOAD) => {
                vec![(profile.join("Extensions").join(&meta.ext), true)]
            }
            (Family::Chromium, ROLE_PREFS) => vec![(profile.join("Preferences"), false)],
            (Family::Firefox, ROLE_PAYLOAD) => vec![
                (
                    profile.join("extensions").join(format!("{}.xpi", meta.ext)),
                    false,
                ),
                (profile.join("extensions").join(&meta.ext), true),
            ],
            (Family::Firefox, ROLE_EXTENSIONS_JSON) => {
                vec![(firefox::extensions_json(profile), false)]
            }
            (Family::Firefox, ROLE_STARTUP) => vec![(firefox::startup_lz4(profile), false)],
            _ => {
                return Err(ApiError::permission_denied(format!(
                    "refusing to restore: unexpected item `{role}` in the backup"
                )))
            }
        };
        let is_dir = match ty {
            "dir" => true,
            "file" => false,
            _ => return Err(ApiError::io("damaged backup: unknown item type")),
        };
        if !expected.iter().any(|(p, d)| *p == original && *d == is_dir) {
            return Err(ApiError::permission_denied(format!(
                "refusing to restore {}: it is outside the browser profile the add-on came from",
                original.display()
            )));
        }
        let rel = s("backup")
            .filter(|r| {
                !r.is_empty()
                    && !r.contains("..")
                    && !r.starts_with('/')
                    && !r.contains('\\')
                    && !r.contains(':')
            })
            .ok_or_else(|| ApiError::io("damaged backup: bad saved-file path"))?;
        let source = backup_dir.join(rel);
        let meta_of = fs::symlink_metadata(&source)
            .map_err(|_| ApiError::io("damaged backup: a saved file is missing"))?;
        if meta_of.file_type().is_symlink() || meta_of.is_dir() != is_dir {
            return Err(ApiError::io(
                "damaged backup: a saved file is not what it says",
            ));
        }
        if out.iter().any(|(r, _)| r == role) {
            return Err(ApiError::io("damaged backup: a file is listed twice"));
        }
        out.push((
            role.to_string(),
            Saved {
                is_dir,
                target: original,
                source,
            },
        ));
    }
    Ok(out)
}

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// Copy the saved payload next to its target, then move it into place.
fn put_payload(saved: &Saved) -> Result<()> {
    let parent = saved
        .target
        .parent()
        .ok_or_else(|| ApiError::io("the add-on has no folder to go into"))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".clearsweep-restore-{}",
        saved
            .target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    if exists(&tmp) {
        if fs::symlink_metadata(&tmp).is_ok_and(|m| m.is_dir()) {
            let _ = fs::remove_dir_all(&tmp);
        } else {
            let _ = fs::remove_file(&tmp);
        }
    }
    let copied = if saved.is_dir {
        copy_dir(&saved.source, &tmp)
    } else {
        fs::copy(&saved.source, &tmp).map(|_| ())
    };
    let done = copied.and_then(|_| fs::rename(&tmp, &saved.target));
    if let Err(e) = done {
        let _ = if saved.is_dir {
            fs::remove_dir_all(&tmp)
        } else {
            fs::remove_file(&tmp)
        };
        return Err(ApiError::io(format!(
            "could not restore the add-on's files: {e}"
        )));
    }
    Ok(())
}

pub fn restore_handler(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    #[derive(serde::Deserialize)]
    struct P {
        id: String,
    }
    let p: P = serde_json::from_value(params)?;
    restore_backup(ctx, &p.id)
}

/// Put the add-on of backup `id` back. See the module docs for the checks.
pub fn restore_backup(ctx: &Ctx, id: &str) -> Result<Value> {
    let (dir, manifest) = read_manifest(ctx, "plugins", id)?;
    let meta = Meta::parse(&manifest)?;
    let def = BROWSERS
        .iter()
        .find(|d| d.key == meta.browser)
        .filter(|d| d.family == meta.family)
        .ok_or_else(|| ApiError::io("damaged backup: unknown browser"))?;
    if is_running(ctx, def) {
        return Err(running_error(def));
    }
    let profile = discover(ctx)
        .into_iter()
        .find(|p| p.def.key == def.key && p.dir == meta.profile_dir)
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "That {} profile no longer exists, so the add-on cannot be put back.",
                def.label
            ))
        })?;
    let items = saved_items(&dir, &manifest, &meta, &profile.dir)?;
    let item = |role: &str| items.iter().find(|(r, _)| r == role).map(|(_, s)| s);
    let payload = item(ROLE_PAYLOAD)
        .ok_or_else(|| ApiError::io("damaged backup: the add-on's files were not saved"))?;

    // Never overwrite: any folder or file of this add-on in the profile wins.
    let in_the_way = match meta.family {
        Family::Chromium => exists(&payload.target),
        Family::Firefox => {
            let base = profile.dir.join("extensions");
            exists(&base.join(format!("{}.xpi", meta.ext))) || exists(&base.join(&meta.ext))
        }
    };
    if in_the_way {
        return Err(ApiError::invalid_params(format!(
            "{} already has {} installed in this profile. It was not replaced; remove that copy first if you want the backed-up one.",
            def.label, meta.name
        )));
    }

    let mut notes: Vec<String> = Vec::new();
    match meta.family {
        Family::Chromium => {
            let files = chromium::ProfileFiles {
                dir: profile.dir.clone(),
            };
            let saved_prefs = match item(ROLE_PREFS) {
                Some(s) => Some(
                    fs::read(&s.source)
                        .ok()
                        .and_then(|b| {
                            serde_json::from_str::<Value>(
                                String::from_utf8_lossy(&b).trim_start_matches('\u{feff}'),
                            )
                            .ok()
                        })
                        .ok_or_else(|| {
                            ApiError::io("damaged backup: the saved Preferences cannot be read")
                        })?,
                ),
                None => None,
            };
            put_payload(payload)?;
            let registered = match saved_prefs {
                Some(v) => match chromium::restore_pref_entry(&files, &meta.ext, &v) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = remove_tree(ctx, &files.extensions(), &payload.target);
                        return Err(e);
                    }
                },
                None => false,
            };
            if !registered {
                notes.push(
                    "The browser keeps its own protected record of this extension. If it does not show up when the browser starts, add it again from the browser's extensions page."
                        .into(),
                );
            }
        }
        Family::Firefox => {
            let ej = item(ROLE_EXTENSIONS_JSON).ok_or_else(|| {
                ApiError::io("damaged backup: the saved extensions.json is missing")
            })?;
            let original = fs::read(firefox::extensions_json(&profile.dir))
                .map_err(|e| ApiError::io(format!("could not read extensions.json: {e}")))?;
            let edit = firefox::compute_restore(
                &profile.dir,
                &meta.ext,
                &ej.source,
                item(ROLE_STARTUP).map(|s| s.source.as_path()),
            )?;
            put_payload(payload)?;
            if let Err(e) = firefox::write_edit(&profile.dir, &edit, &original) {
                let _ = remove_tree(ctx, &profile.dir.join("extensions"), &payload.target);
                return Err(e);
            }
        }
    }
    Ok(json!({
        "ok": true,
        "id": id,
        "name": meta.name,
        "browser": def.label,
        "restored": 1,
        "notes": notes,
    }))
}
