//! Browser plugins: list, enable, disable and remove browser extensions.
//!
//! Methods:
//! - `browser_plugins.list`: extensions, themes, apps and dictionaries of Chrome, Chromium,
//!   Edge, Brave, Opera, Vivaldi and Firefox, per profile.
//! - `browser_plugins.set_enabled { id, enabled }` and `browser_plugins.remove { id }`: the
//!   browser must be closed (otherwise it would overwrite the change on exit), the item is
//!   looked up again on the server, and a backup goes to `<data>/backups/plugins-<ts>/`
//!   first.
//!
//! - `browser_plugins.restore_backup { id }`: put a removed add-on back from its
//!   `plugins-<ts>` backup (see [`restore`]); the browser must be closed and nothing is
//!   overwritten.
//!
//! Chromium: only entries in the plain `Preferences` file are edited. Extension settings in
//! `Secure Preferences` are protected by HMACs and are never touched (`canDisable: false`
//! plus a note). Firefox: `extensions.json` and `addonStartup.json.lz4` are edited together.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::api::Registry;
use crate::ctx::{Ctx, Os};
use crate::error::{ApiError, Result};
use crate::features::cleaner::firefox as ff_profiles;
use crate::features::startup::backup::Backup;
use crate::job::Job;
use crate::procs::name_matches;
use crate::safety::{ExcludeSet, SafeDeleter};

pub mod chromium;
pub mod firefox;
pub mod mozlz4;
pub mod restore;

#[cfg(test)]
mod tests;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "browser_plugins.list",
    "browser_plugins.set_enabled",
    "browser_plugins.remove",
    "browser_plugins.restore_backup",
];

pub fn register(r: &mut Registry) {
    r.add("browser_plugins.list", list_handler);
    r.add("browser_plugins.set_enabled", set_enabled_handler);
    r.add("browser_plugins.remove", remove_handler);
    r.add("browser_plugins.restore_backup", restore::restore_handler);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plugin {
    /// `browser:profile:extensionId`
    pub id: String,
    pub browser: String,
    pub browser_label: String,
    pub profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    pub extension_id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub enabled: bool,
    /// `extension`, `theme`, `app`, `plugin`, `dictionary` or `locale`.
    #[serde(rename = "type")]
    pub kind: String,
    pub install_location: String,
    pub can_disable: bool,
    pub can_remove: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The browser is running right now (changes need it closed).
    pub running: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Chromium,
    Firefox,
}

pub struct BrowserDef {
    pub key: &'static str,
    pub label: &'static str,
    pub family: Family,
    linux: &'static [&'static str],
    windows: &'static [&'static str],
    macos: &'static [&'static str],
}

pub const BROWSERS: &[BrowserDef] = &[
    BrowserDef {
        key: "chrome",
        label: "Google Chrome",
        family: Family::Chromium,
        linux: &["chrome", "google-chrome", "google-chrome-stable"],
        windows: &["chrome.exe"],
        macos: &["Google Chrome"],
    },
    BrowserDef {
        key: "chromium",
        label: "Chromium",
        family: Family::Chromium,
        linux: &["chromium", "chromium-browser"],
        windows: &["chrome.exe"],
        macos: &["Chromium"],
    },
    BrowserDef {
        key: "edge",
        label: "Microsoft Edge",
        family: Family::Chromium,
        linux: &["msedge", "microsoft-edge", "microsoft-edge-stable"],
        windows: &["msedge.exe"],
        macos: &["Microsoft Edge"],
    },
    BrowserDef {
        key: "brave",
        label: "Brave",
        family: Family::Chromium,
        linux: &["brave", "brave-browser", "brave-browser-stable"],
        windows: &["brave.exe"],
        macos: &["Brave Browser"],
    },
    BrowserDef {
        key: "opera",
        label: "Opera",
        family: Family::Chromium,
        linux: &["opera"],
        windows: &["opera.exe"],
        macos: &["Opera"],
    },
    BrowserDef {
        key: "vivaldi",
        label: "Vivaldi",
        family: Family::Chromium,
        linux: &["vivaldi", "vivaldi-bin", "vivaldi-stable"],
        windows: &["vivaldi.exe"],
        macos: &["Vivaldi"],
    },
    BrowserDef {
        key: "firefox",
        label: "Firefox",
        family: Family::Firefox,
        linux: &["firefox", "firefox-bin", "firefox-esr", "firefox.real"],
        windows: &["firefox.exe"],
        macos: &["firefox"],
    },
];

impl BrowserDef {
    fn process_names(&self, os: Os) -> &'static [&'static str] {
        match os {
            Os::Linux => self.linux,
            Os::Windows => self.windows,
            Os::MacOs => self.macos,
        }
    }
}

pub fn is_running(ctx: &Ctx, def: &BrowserDef) -> bool {
    let want = def.process_names(ctx.env.os);
    ctx.procs
        .list()
        .iter()
        .any(|p| want.iter().any(|w| name_matches(&p.name, w)))
}

fn running_error(def: &BrowserDef) -> ApiError {
    ApiError::permission_denied(format!(
        "Close {} first: while it is running it would undo this change.",
        def.label
    ))
}

/// One browser profile on disk.
pub struct ProfileRef {
    pub def: &'static BrowserDef,
    /// Stable label used in ids (`Default`, `Profile 1`, `abc.default-release`...).
    pub label: String,
    pub dir: PathBuf,
}

pub fn discover(ctx: &Ctx) -> Vec<ProfileRef> {
    let mut out = Vec::new();
    for def in BROWSERS {
        match def.family {
            Family::Chromium => {
                let mut with_profiles = 0usize;
                for root in chromium::roots(ctx, def.key) {
                    let profiles = chromium::profile_dirs(&root);
                    if profiles.is_empty() {
                        continue;
                    }
                    let suffix = if with_profiles > 0 {
                        format!(
                            "@{}",
                            root.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        )
                    } else {
                        String::new()
                    };
                    with_profiles += 1;
                    for dir in profiles {
                        let name = if dir == root {
                            "Default".to_string()
                        } else {
                            dir.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        };
                        out.push(ProfileRef {
                            def,
                            label: format!("{name}{suffix}"),
                            dir,
                        });
                    }
                }
            }
            Family::Firefox => {
                for dir in ff_profiles::profile_dirs(&ctx.env) {
                    let label = dir
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    out.push(ProfileRef { def, label, dir });
                }
            }
        }
    }
    out
}

fn list_one(ctx: &Ctx, p: &ProfileRef, running: bool) -> Vec<Plugin> {
    match p.def.family {
        Family::Chromium => chromium::list_profile(
            p.def.key,
            p.def.label,
            &p.label,
            chromium::profile_display_name(&p.dir),
            &chromium::ProfileFiles { dir: p.dir.clone() },
            running,
        ),
        Family::Firefox => {
            let _ = ctx;
            firefox::list_profile(p.def.key, p.def.label, &p.dir, &p.label, running)
        }
    }
}

pub fn list_all(ctx: &Ctx) -> Vec<Plugin> {
    let mut running_cache: Vec<(&str, bool)> = Vec::new();
    let mut out = Vec::new();
    for p in discover(ctx) {
        let running = match running_cache.iter().find(|(k, _)| *k == p.def.key) {
            Some((_, r)) => *r,
            None => {
                let r = is_running(ctx, p.def);
                running_cache.push((p.def.key, r));
                r
            }
        };
        out.extend(list_one(ctx, &p, running));
    }
    out.sort_by(|a, b| {
        (&a.browser_label, &a.profile, a.name.to_lowercase()).cmp(&(
            &b.browser_label,
            &b.profile,
            b.name.to_lowercase(),
        ))
    });
    out
}

fn list_handler(ctx: &Ctx, _params: Value, job: &Job) -> Result<Value> {
    job.check_cancelled()?;
    Ok(serde_json::to_value(list_all(ctx))?)
}

// ---------------------------------------------------------------- id handling

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct SetParams {
    id: String,
    enabled: bool,
}

/// `browser:profile:ext` -> the profile and the plugin, freshly listed.
fn resolve(ctx: &Ctx, id: &str) -> Result<(ProfileRef, Plugin, bool)> {
    let (browser, rest) = id
        .split_once(':')
        .ok_or_else(|| ApiError::invalid_params("malformed plugin id"))?;
    let (profile_label, ext) = rest
        .rsplit_once(':')
        .ok_or_else(|| ApiError::invalid_params("malformed plugin id"))?;
    if !chromium::valid_ext_id(ext) {
        return Err(ApiError::invalid_params("malformed plugin id"));
    }
    let p = discover(ctx)
        .into_iter()
        .find(|p| p.def.key == browser && p.label == profile_label)
        .ok_or_else(|| ApiError::not_found("that browser profile no longer exists"))?;
    let running = is_running(ctx, p.def);
    let plugin = list_one(ctx, &p, running)
        .into_iter()
        .find(|x| x.extension_id == ext)
        .ok_or_else(|| ApiError::not_found("that add-on no longer exists"))?;
    Ok((p, plugin, running))
}

// ---------------------------------------------------------------- set_enabled

fn set_enabled_handler(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: SetParams = serde_json::from_value(params)?;
    let (profile, plugin, running) = resolve(ctx, &p.id)?;
    if running {
        return Err(running_error(profile.def));
    }
    if !plugin.can_disable {
        return Err(ApiError::unsupported(
            plugin
                .note
                .clone()
                .unwrap_or_else(|| "This add-on cannot be changed here".into()),
        ));
    }
    if plugin.enabled == p.enabled {
        return Ok(json!({ "ok": true, "plugin": plugin }));
    }
    let desc = format!(
        "{} {} in {}",
        if p.enabled { "Enabled" } else { "Disabled" },
        plugin.name,
        profile.def.label
    );
    let mut b = Backup::create(ctx, "plugins", &desc)?;
    b.set_field("plugin", restore::meta(&profile, &plugin, "set_enabled"));
    match profile.def.family {
        Family::Chromium => {
            let files = chromium::ProfileFiles {
                dir: profile.dir.clone(),
            };
            if let Err(e) = stage_file(&mut b, &files.prefs(), restore::ROLE_PREFS) {
                b.abort();
                return Err(e);
            }
            let done = b.commit()?;
            chromium::set_enabled(&files, &plugin.extension_id, p.enabled)?;
            let after = resolve(ctx, &p.id).map(|(_, x, _)| x).unwrap_or(plugin);
            Ok(json!({ "ok": true, "plugin": after, "backupId": done.id }))
        }
        Family::Firefox => {
            let ejson = firefox::extensions_json(&profile.dir);
            let original = fs::read(&ejson)?;
            let edit = match firefox::compute_edit(
                &profile.dir,
                &plugin.extension_id,
                |a| firefox::set_flags(a, p.enabled),
                p.enabled,
            ) {
                Ok(e) => e,
                Err(e) => {
                    b.abort();
                    return Err(e);
                }
            };
            for (f, role) in [
                (ejson.clone(), restore::ROLE_EXTENSIONS_JSON),
                (firefox::startup_lz4(&profile.dir), restore::ROLE_STARTUP),
            ] {
                if f.exists() {
                    if let Err(e) = stage_file(&mut b, &f, role) {
                        b.abort();
                        return Err(e);
                    }
                }
            }
            let done = b.commit()?;
            firefox::write_edit(&profile.dir, &edit, &original)?;
            let after = resolve(ctx, &p.id).map(|(_, x, _)| x).unwrap_or(plugin);
            Ok(json!({ "ok": true, "plugin": after, "backupId": done.id }))
        }
    }
}

fn stage_file(b: &mut Backup, src: &Path, role: &str) -> Result<()> {
    let copy = b.copy_file(src)?;
    let rel = b.rel(&copy);
    b.add_item(json!({
        "type": "file",
        "role": role,
        "original": src.to_string_lossy(),
        "backup": rel,
    }));
    Ok(())
}

fn stage_tree(b: &mut Backup, src: &Path) -> Result<()> {
    let copy = if src.is_dir() {
        b.copy_tree(src)?
    } else {
        b.copy_file(src)?
    };
    let rel = b.rel(&copy);
    b.add_item(json!({
        "type": if src.is_dir() { "dir" } else { "file" },
        "role": restore::ROLE_PAYLOAD,
        "original": src.to_string_lossy(),
        "backup": rel,
    }));
    Ok(())
}

// ---------------------------------------------------------------- remove

/// Delete a file, or a directory tree, below `base` through [`SafeDeleter`].
pub fn remove_tree(ctx: &Ctx, base: &Path, target: &Path) -> Result<()> {
    let d = SafeDeleter::new(&ctx.env, base, ExcludeSet::empty())?;
    let meta = fs::symlink_metadata(target)?;
    if !meta.file_type().is_dir() {
        d.remove_file(target).map_err(|e| e.to_api(target))?;
        return Ok(());
    }
    for e in WalkDir::new(target)
        .follow_links(false)
        .contents_first(true)
    {
        let e = e.map_err(|e| ApiError::io(e.to_string()))?;
        if e.file_type().is_dir() {
            d.remove_empty_dir(e.path())
                .map_err(|x| x.to_api(e.path()))?;
        } else {
            d.remove_file(e.path()).map_err(|x| x.to_api(e.path()))?;
        }
    }
    Ok(())
}

fn remove_handler(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: IdParams = serde_json::from_value(params)?;
    let (profile, plugin, running) = resolve(ctx, &p.id)?;
    if running {
        return Err(running_error(profile.def));
    }
    if !plugin.can_remove {
        return Err(ApiError::unsupported(
            "This add-on cannot be removed here; remove it from the browser's own page",
        ));
    }
    let desc = format!(
        "Removed browser add-on: {} ({})",
        plugin.name, profile.def.label
    );
    let mut b = Backup::create(ctx, "plugins", &desc)?;
    b.set_field("plugin", restore::meta(&profile, &plugin, "remove"));
    match profile.def.family {
        Family::Chromium => {
            let files = chromium::ProfileFiles {
                dir: profile.dir.clone(),
            };
            let ext_dir = files.extensions().join(&plugin.extension_id);
            let staged = stage_tree(&mut b, &ext_dir).and_then(|_| {
                if files.prefs().is_file() {
                    stage_file(&mut b, &files.prefs(), restore::ROLE_PREFS)
                } else {
                    Ok(())
                }
            });
            if let Err(e) = staged {
                b.abort();
                return Err(e);
            }
            let done = b.commit()?;
            let location = chromium::entry_location(&files, &plugin.extension_id);
            let mut note = None;
            if location == Some("Preferences") {
                chromium::remove_pref_entry(&files, &plugin.extension_id)?;
            } else if location == Some("Secure Preferences") {
                note = Some(
                    "The browser keeps its protected record of this extension and will clear it the next time it starts.",
                );
            }
            remove_tree(ctx, &files.extensions(), &ext_dir)?;
            Ok(
                json!({ "ok": true, "backupId": done.id, "backupPath": done.dir.to_string_lossy(), "note": note }),
            )
        }
        Family::Firefox => {
            let payload = firefox::addon_payload(&profile.dir, &plugin.extension_id)
                .ok_or_else(|| ApiError::not_found("the add-on's file is not in this profile"))?;
            let ejson = firefox::extensions_json(&profile.dir);
            let original = fs::read(&ejson)?;
            let edit = match firefox::compute_edit(
                &profile.dir,
                &plugin.extension_id,
                firefox::set_inactive,
                false,
            ) {
                Ok(e) => e,
                Err(e) => {
                    b.abort();
                    return Err(e);
                }
            };
            let mut staged = stage_tree(&mut b, &payload);
            for (f, role) in [
                (ejson.clone(), restore::ROLE_EXTENSIONS_JSON),
                (firefox::startup_lz4(&profile.dir), restore::ROLE_STARTUP),
            ] {
                if staged.is_ok() && f.exists() {
                    staged = stage_file(&mut b, &f, role);
                }
            }
            if let Err(e) = staged {
                b.abort();
                return Err(e);
            }
            let done = b.commit()?;
            firefox::write_edit(&profile.dir, &edit, &original)?;
            remove_tree(ctx, &profile.dir.join("extensions"), &payload)?;
            Ok(json!({
                "ok": true,
                "backupId": done.id,
                "backupPath": done.dir.to_string_lossy(),
                "note": "Firefox finishes the removal the next time it starts.",
            }))
        }
    }
}
