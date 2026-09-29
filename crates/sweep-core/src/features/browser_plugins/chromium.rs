//! Chromium-family extensions (Chrome, Chromium, Edge, Brave, Opera, Vivaldi).
//!
//! Extension settings live in `extensions.settings.<id>` of the profile's `Preferences` or
//! `Secure Preferences` JSON. Entries in `Secure Preferences` (and any entry with a
//! `protection.macs` record) are MAC-protected: editing them would make the browser reset
//! them, so those are never edited (`canDisable` is false with an explanatory note).

use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ctx::{Ctx, Os};
use crate::error::{ApiError, Result};
use crate::fsutil::atomic_write;

use super::Plugin;

pub const NOTE_PROTECTED: &str =
    "This browser protects extension settings; disable it from the browser's extensions page";
pub const NOTE_POLICY: &str = "Installed by your organisation's policy; it cannot be changed here";
pub const NOTE_UNPACKED: &str =
    "Loaded from a folder on disk; remove it from the browser's extensions page";

/// `Manifest::Location` values that are never listed (built into the browser).
const LOC_COMPONENT: i64 = 5;
const LOC_EXTERNAL_COMPONENT: i64 = 10;
/// Locations installed by an administrator policy.
const POLICY_LOCATIONS: &[i64] = &[7, 9];
const LOC_UNPACKED: i64 = 4;
const LOC_COMMAND_LINE: i64 = 8;

/// Extensions that ship with the browser without being "component" extensions.
const HIDDEN_IDS: &[&str] = &[
    "nmmhkkegccagdldgiimedpiccmgmieda", // Chrome Web Store Payments
    "ahfgeienlihckogmohjhadlkjgocpleb", // Chrome Web Store
    "mhjfbmdgcfjbbpaeojofohoefgiehjai", // PDF viewer
    "pkedcjkdefgpdelpbcmbmeomcjbeemfm", // Cast
    "gfdkimpbcpahaombhbimeihdjnejgicl", // feedback
    "neajdppkdcdipfabeoofebfddakdcjhd", // network speech
];

/// User-data roots of a Chromium-family browser on this OS.
pub fn roots(ctx: &Ctx, key: &str) -> Vec<PathBuf> {
    let e = &ctx.env;
    let home = &e.home;
    match (e.os, key) {
        (Os::Linux, "chrome") => vec![
            e.config_dir.join("google-chrome"),
            e.config_dir.join("google-chrome-beta"),
            home.join(".var/app/com.google.Chrome/config/google-chrome"),
        ],
        (Os::Linux, "chromium") => vec![
            e.config_dir.join("chromium"),
            home.join("snap/chromium/common/chromium"),
            home.join(".var/app/org.chromium.Chromium/config/chromium"),
        ],
        (Os::Linux, "edge") => vec![
            e.config_dir.join("microsoft-edge"),
            e.config_dir.join("microsoft-edge-beta"),
            home.join(".var/app/com.microsoft.Edge/config/microsoft-edge"),
        ],
        (Os::Linux, "brave") => vec![
            e.config_dir.join("BraveSoftware/Brave-Browser"),
            home.join(".var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser"),
            home.join("snap/brave/current/.config/BraveSoftware/Brave-Browser"),
        ],
        (Os::Linux, "opera") => vec![
            e.config_dir.join("opera"),
            home.join(".var/app/com.opera.Opera/config/opera"),
        ],
        (Os::Linux, "vivaldi") => vec![
            e.config_dir.join("vivaldi"),
            e.config_dir.join("vivaldi-snapshot"),
            home.join(".var/app/com.vivaldi.Vivaldi/config/vivaldi"),
        ],
        (Os::Windows, "chrome") => vec![e.data_local_dir.join("Google/Chrome/User Data")],
        (Os::Windows, "chromium") => vec![e.data_local_dir.join("Chromium/User Data")],
        (Os::Windows, "edge") => vec![e.data_local_dir.join("Microsoft/Edge/User Data")],
        (Os::Windows, "brave") => {
            vec![e.data_local_dir.join("BraveSoftware/Brave-Browser/User Data")]
        }
        (Os::Windows, "opera") => vec![e.config_dir.join("Opera Software/Opera Stable")],
        (Os::Windows, "vivaldi") => vec![e.data_local_dir.join("Vivaldi/User Data")],
        (Os::MacOs, "chrome") => vec![e.config_dir.join("Google/Chrome")],
        (Os::MacOs, "chromium") => vec![e.config_dir.join("Chromium")],
        (Os::MacOs, "edge") => vec![e.config_dir.join("Microsoft Edge")],
        (Os::MacOs, "brave") => vec![e.config_dir.join("BraveSoftware/Brave-Browser")],
        (Os::MacOs, "opera") => vec![e.config_dir.join("com.operasoftware.Opera")],
        (Os::MacOs, "vivaldi") => vec![e.config_dir.join("Vivaldi")],
        _ => Vec::new(),
    }
}

/// Profile directories below a user-data root: `Default`, `Profile N`, ... (any real
/// directory holding a `Preferences` file). Opera keeps its single profile in the root.
pub fn profile_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if root.join("Preferences").is_file() && root.join("Extensions").exists() {
        out.push(root.to_path_buf());
    }
    if let Ok(rd) = fs::read_dir(root) {
        let mut subs: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().map(|n| n.to_string_lossy().into_owned());
                n.is_some_and(|n| {
                    n != "System Profile"
                        && n != "Guest Profile"
                        && (n == "Default" || n.starts_with("Profile "))
                }) && p.join("Preferences").is_file()
            })
            .collect();
        subs.sort();
        out.extend(subs);
    }
    out
}

fn read_json(p: &Path) -> Option<Value> {
    let bytes = fs::read(p).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

fn settings_of(v: &Value) -> Option<&Map<String, Value>> {
    v.get("extensions")?.get("settings")?.as_object()
}

fn has_mac(v: &Value, id: &str) -> bool {
    v.get("protection")
        .and_then(|p| p.get("macs"))
        .and_then(|m| m.get("extensions"))
        .and_then(|e| e.get("settings"))
        .and_then(|s| s.get(id))
        .is_some()
}

/// Display name of the profile (`profile.name` in `Preferences`), if any.
pub fn profile_display_name(profile: &Path) -> Option<String> {
    let v = read_json(&profile.join("Preferences"))?;
    v.get("profile")?
        .get("name")?
        .as_str()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

fn disable_mask(entry: &Value) -> i64 {
    match entry.get("disable_reasons") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_i64).fold(0, |m, r| m | r),
        _ => 0,
    }
}

/// Enabled: no `disable_reasons` and not `state: 0`. Both the old format (`state` only /
/// bitmask) and the new one (array or bitmask `disable_reasons`) are understood.
pub fn entry_enabled(entry: &Value) -> bool {
    if entry.get("state").and_then(Value::as_i64) == Some(0) {
        return false;
    }
    disable_mask(entry) == 0
}

// ---------------------------------------------------------------- manifests

fn resolve_msg(dir: &Path, default_locale: Option<&str>, text: &str) -> String {
    let Some(key) = text
        .strip_prefix("__MSG_")
        .and_then(|t| t.strip_suffix("__"))
    else {
        return text.to_string();
    };
    let mut locales: Vec<String> = Vec::new();
    if let Some(l) = default_locale {
        locales.push(l.to_string());
    }
    locales.extend(["en".into(), "en_US".into(), "en_GB".into()]);
    for loc in locales {
        let p = dir.join("_locales").join(&loc).join("messages.json");
        let Some(v) = read_json(&p) else { continue };
        let Some(obj) = v.as_object() else { continue };
        // Message keys are case-insensitive.
        if let Some((_, m)) = obj.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            if let Some(s) = m.get("message").and_then(Value::as_str) {
                return s.to_string();
            }
        }
    }
    text.to_string()
}

#[derive(Debug, Default)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub kind: &'static str,
}

/// `manifest.json` allows `//` and `/* */` comments; strip them outside strings.
fn strip_json_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            c => out.push(c),
        }
    }
    out
}

pub fn read_manifest(dir: &Path, embedded: Option<&Value>) -> Option<Manifest> {
    let parsed = fs::read(dir.join("manifest.json")).ok().and_then(|b| {
        let t = String::from_utf8_lossy(&b).into_owned();
        let t = t.trim_start_matches('\u{feff}');
        serde_json::from_str::<Value>(t)
            .or_else(|_| serde_json::from_str::<Value>(&strip_json_comments(t)))
            .ok()
    });
    let m = parsed.as_ref().or(embedded)?;
    let s = |k: &str| m.get(k).and_then(Value::as_str);
    let locale = s("default_locale");
    let kind = if m.get("theme").is_some() {
        "theme"
    } else if m.get("app").is_some() {
        "app"
    } else if m.get("language").is_some() {
        "locale"
    } else {
        "extension"
    };
    Some(Manifest {
        name: resolve_msg(dir, locale, s("name")?),
        version: s("version_name")
            .or_else(|| s("version"))
            .unwrap_or_default()
            .to_string(),
        description: s("description")
            .map(|d| resolve_msg(dir, locale, d))
            .unwrap_or_default(),
        kind,
    })
}

// ---------------------------------------------------------------- listing

/// Where one profile keeps its extension data.
pub struct ProfileFiles {
    pub dir: PathBuf,
}

impl ProfileFiles {
    pub fn prefs(&self) -> PathBuf {
        self.dir.join("Preferences")
    }
    pub fn secure(&self) -> PathBuf {
        self.dir.join("Secure Preferences")
    }
    pub fn extensions(&self) -> PathBuf {
        self.dir.join("Extensions")
    }
}

fn is_absolute_path(p: &str) -> bool {
    p.starts_with('/') || p.starts_with('\\') || p.as_bytes().get(1) == Some(&b':')
}

/// All extensions of one profile.
pub fn list_profile(
    browser: &'static str,
    browser_label: &str,
    profile_label: &str,
    profile_name: Option<String>,
    files: &ProfileFiles,
    running: bool,
) -> Vec<Plugin> {
    let prefs = read_json(&files.prefs());
    let secure = read_json(&files.secure());
    let mut ids: Vec<String> = Vec::new();
    for v in [&prefs, &secure].into_iter().flatten() {
        if let Some(s) = settings_of(v) {
            ids.extend(s.keys().cloned());
        }
    }
    ids.sort();
    ids.dedup();
    let mut out = Vec::new();
    for id in ids {
        if !valid_ext_id(&id) || HIDDEN_IDS.contains(&id.as_str()) {
            continue;
        }
        let in_secure = secure
            .as_ref()
            .and_then(settings_of)
            .is_some_and(|s| s.contains_key(&id));
        let in_prefs = prefs
            .as_ref()
            .and_then(settings_of)
            .is_some_and(|s| s.contains_key(&id));
        // The secure copy wins when both exist.
        let (entry, file_value) = if in_secure {
            (
                secure.as_ref().and_then(settings_of).and_then(|s| s.get(&id)),
                secure.as_ref(),
            )
        } else if in_prefs {
            (
                prefs.as_ref().and_then(settings_of).and_then(|s| s.get(&id)),
                prefs.as_ref(),
            )
        } else {
            continue;
        };
        let (Some(entry), Some(file_value)) = (entry, file_value) else {
            continue;
        };
        let location = entry.get("location").and_then(Value::as_i64).unwrap_or(1);
        if location == LOC_COMPONENT || location == LOC_EXTERNAL_COMPONENT {
            continue;
        }
        let rel_path = entry.get("path").and_then(Value::as_str).unwrap_or("");
        let dir = if rel_path.is_empty() {
            files.extensions().join(&id)
        } else if is_absolute_path(rel_path) {
            PathBuf::from(rel_path)
        } else {
            // Relative paths always stay inside Extensions/.
            let mut p = files.extensions();
            for c in rel_path.replace('\\', "/").split('/') {
                if !c.is_empty() && c != "." && c != ".." {
                    p.push(c);
                }
            }
            p
        };
        let Some(m) = read_manifest(&dir, entry.get("manifest")) else {
            // No manifest anywhere: an entry the browser itself will clean up.
            continue;
        };
        let policy = POLICY_LOCATIONS.contains(&location);
        let unpacked = location == LOC_UNPACKED || location == LOC_COMMAND_LINE;
        let protected = in_secure || has_mac(file_value, &id);
        let mut note: Option<String> = None;
        let mut can_disable = true;
        if policy {
            can_disable = false;
            note = Some(NOTE_POLICY.into());
        } else if protected {
            can_disable = false;
            note = Some(NOTE_PROTECTED.into());
        }
        let ext_dir = files.extensions().join(&id);
        let can_remove = !policy && !unpacked && matches!(location, 1 | 2 | 3 | 6) && ext_dir.is_dir();
        if unpacked && note.is_none() {
            note = Some(NOTE_UNPACKED.into());
        }
        out.push(Plugin {
            id: format!("{browser}:{profile_label}:{id}"),
            browser: browser.to_string(),
            browser_label: browser_label.to_string(),
            profile: profile_label.to_string(),
            profile_name: profile_name.clone(),
            extension_id: id.clone(),
            name: m.name,
            version: m.version,
            description: m.description,
            enabled: entry_enabled(entry),
            kind: m.kind.to_string(),
            install_location: dir.to_string_lossy().into_owned(),
            can_disable,
            can_remove,
            note,
            running,
        });
    }
    out
}

/// Extension ids are 32 chars a-p for the web store, but unpacked ones and other stores
/// differ; the only thing that matters is that the id is safe as a path component / key.
pub fn valid_ext_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && !id.contains(['/', '\\', '\0', ':'])
}

// ---------------------------------------------------------------- editing

/// The `extensions.settings.<id>` object in `prefs`, mutably.
fn entry_mut<'a>(prefs: &'a mut Value, id: &str) -> Option<&'a mut Map<String, Value>> {
    prefs
        .get_mut("extensions")?
        .get_mut("settings")?
        .get_mut(id)?
        .as_object_mut()
}

/// Set an entry's enabled state in place. Every other key is left as it is.
pub fn apply_enabled_to_entry(entry: &mut Map<String, Value>, enabled: bool) -> Result<()> {
    const USER_ACTION: i64 = 1;
    if enabled {
        match entry.get_mut("disable_reasons") {
            Some(Value::Array(a)) => {
                a.retain(|r| r.as_i64() != Some(USER_ACTION));
                if a.is_empty() {
                    entry.remove("disable_reasons");
                }
            }
            Some(Value::Number(n)) => {
                let rest = n.as_i64().unwrap_or(0) & !USER_ACTION;
                if rest == 0 {
                    entry.remove("disable_reasons");
                } else {
                    entry.insert("disable_reasons".into(), Value::from(rest));
                }
            }
            _ => {}
        }
        let still = disable_mask(&Value::Object(entry.clone()));
        if still != 0 {
            return Err(ApiError::unsupported(
                "The browser disabled this extension for another reason (for example a permission change); enable it from the browser's extensions page",
            ));
        }
        if entry.contains_key("state") {
            entry.insert("state".into(), Value::from(1));
        }
    } else {
        match entry.get_mut("disable_reasons") {
            Some(Value::Array(a)) => {
                if !a.iter().any(|r| r.as_i64() == Some(USER_ACTION)) {
                    a.push(Value::from(USER_ACTION));
                }
            }
            Some(Value::Number(n)) => {
                let m = n.as_i64().unwrap_or(0) | USER_ACTION;
                entry.insert("disable_reasons".into(), Value::from(m));
            }
            _ => {
                entry.insert("disable_reasons".into(), Value::from(USER_ACTION));
            }
        }
        // Old browsers only look at `state`; new ones ignore it.
        entry.insert("state".into(), Value::from(0));
    }
    Ok(())
}

fn write_json(path: &Path, v: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(v)?;
    atomic_write(path, &bytes)
        .map_err(|e| ApiError::io(format!("could not write {}: {e}", path.display())))
}

/// Edit `extensions.settings.<id>` in the (non-secure) `Preferences` file.
pub fn set_enabled(files: &ProfileFiles, id: &str, enabled: bool) -> Result<()> {
    let path = files.prefs();
    let mut prefs = read_json(&path)
        .ok_or_else(|| ApiError::io(format!("could not read {}", path.display())))?;
    if has_mac(&prefs, id) {
        return Err(ApiError::unsupported(NOTE_PROTECTED));
    }
    let entry = entry_mut(&mut prefs, id)
        .ok_or_else(|| ApiError::not_found("the extension is not in the browser's Preferences"))?;
    apply_enabled_to_entry(entry, enabled)?;
    write_json(&path, &prefs)
}

/// Remove `extensions.settings.<id>` (and its MAC record) from `Preferences` if it is there.
/// Returns whether anything was removed.
pub fn remove_pref_entry(files: &ProfileFiles, id: &str) -> Result<bool> {
    let path = files.prefs();
    let Some(mut prefs) = read_json(&path) else {
        return Ok(false);
    };
    let removed = prefs
        .get_mut("extensions")
        .and_then(|e| e.get_mut("settings"))
        .and_then(Value::as_object_mut)
        .map(|s| s.remove(id).is_some())
        .unwrap_or(false);
    if !removed {
        return Ok(false);
    }
    if let Some(m) = prefs
        .get_mut("protection")
        .and_then(|p| p.get_mut("macs"))
        .and_then(|m| m.get_mut("extensions"))
        .and_then(|e| e.get_mut("settings"))
        .and_then(Value::as_object_mut)
    {
        m.remove(id);
    }
    write_json(&path, &prefs)?;
    Ok(true)
}

/// Where the settings entry of `id` lives: `"Preferences"`, `"Secure Preferences"` or `None`.
pub fn entry_location(files: &ProfileFiles, id: &str) -> Option<&'static str> {
    let has = |p: PathBuf| {
        read_json(&p)
            .as_ref()
            .and_then(settings_of)
            .is_some_and(|s| s.contains_key(id))
    };
    if has(files.secure()) {
        Some("Secure Preferences")
    } else if has(files.prefs()) {
        Some("Preferences")
    } else {
        None
    }
}
