//! Settings: persistent app settings stored in `<data_dir>/settings.json`.
//!
//! Methods:
//! - `settings.get`: the full settings object.
//! - `settings.set`: RFC 7386 merge-patch, validated; returns the new settings. Setting
//!   `runAtStartup` installs / removes the OS autostart entry (`crate::autostart`) and is
//!   refused, with nothing saved, when that fails.
//! - `settings.reset`: restore defaults (and remove the autostart entry).
//!
//! Adding a setting later: add a field to [`Settings`] (with a default) and, if it needs
//! validation, one arm in [`Settings::validate_field`]. Old files keep loading (every
//! field is optional) and unknown keys in a file are ignored.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, PathBuf};
use std::sync::Mutex;

use crate::api::Registry;
use crate::ctx::{Ctx, Env};
use crate::error::{ApiError, Result};
use crate::features::cookies::normalize_domain;
use crate::fsutil::{atomic_write, now_unix, random_id};
use crate::job::Job;
use crate::safety::{expand_tilde, normalize, validate_exclude_pattern, Protected};

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["settings.get", "settings.set", "settings.reset"];

pub fn register(r: &mut Registry) {
    r.add("settings.get", get);
    r.add("settings.set", set);
    r.add("settings.reset", reset);
}

const MAX_LIST: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CloseBrowsers {
    #[default]
    Ask,
    Always,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SecureDeleteSettings {
    pub enabled: bool,
    /// One of 1, 3, 7, 35.
    pub passes: u32,
}

impl Default for SecureDeleteSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            passes: 1,
        }
    }
}

fn default_mask() -> String {
    "*".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct IncludeEntry {
    pub id: String,
    pub path: String,
    pub recursive: bool,
    #[serde(default = "default_mask")]
    pub mask: String,
    pub remove_empty_dirs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ExcludeEntry {
    pub id: String,
    /// Absolute path or glob; `~` is expanded.
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SmartSettings {
    pub enabled: bool,
    pub threshold_mb: u32,
    pub clean_on_browser_close: Vec<String>,
    pub auto_clean: bool,
    pub notify: bool,
    /// How often the background agent measures the junk (minutes, 5 or more).
    pub check_interval_minutes: u32,
    /// How often the background agent re-applies sleep mode (minutes, 1 or more).
    pub enforce_sleep_minutes: u32,
}

impl Default for SmartSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_mb: 500,
            clean_on_browser_close: Vec::new(),
            auto_clean: false,
            notify: true,
            check_interval_minutes: 60,
            enforce_sleep_minutes: 15,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub theme: Theme,
    pub secure_delete: SecureDeleteSettings,
    pub close_browsers: CloseBrowsers,
    /// Temp files younger than this are never removed.
    pub temp_min_age_hours: u32,
    pub include: Vec<IncludeEntry>,
    pub exclude: Vec<ExcludeEntry>,
    /// Domains whose cookies are kept by cookie cleaning.
    pub cookie_keep: Vec<String>,
    /// `None` = every rule's own default.
    pub selected_rules: Option<Vec<String>>,
    pub smart: SmartSettings,
    /// Launch the background agent at login (see `crate::autostart`).
    pub run_at_startup: bool,
    /// Desktop app: the close button hides the window to the tray.
    pub close_to_tray: bool,
    pub language: String,
    /// Software updater: ids (`apt:firefox`, `winget:Git.Git`, ...) the user chose to ignore.
    pub ignored_updates: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            secure_delete: SecureDeleteSettings::default(),
            close_browsers: CloseBrowsers::Ask,
            temp_min_age_hours: 24,
            include: Vec::new(),
            exclude: Vec::new(),
            cookie_keep: Vec::new(),
            selected_rules: None,
            smart: SmartSettings::default(),
            run_at_startup: false,
            close_to_tray: true,
            language: "en".to_string(),
            ignored_updates: Vec::new(),
        }
    }
}

fn err(msg: impl Into<String>) -> ApiError {
    ApiError::invalid_params(msg)
}

fn unique_ids<'a>(what: &str, ids: impl Iterator<Item = &'a String>) -> Result<()> {
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id.as_str()) {
            return Err(err(format!("duplicate {what} id `{id}`")));
        }
    }
    Ok(())
}

/// Expand `~`, require an absolute path without `..`, and normalize it.
pub fn resolve_user_path(env: &Env, raw: &str) -> Result<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(err("path is empty"));
    }
    let p = expand_tilde(env, raw);
    if !p.is_absolute() {
        return Err(err(format!("path must be absolute or start with ~: {raw}")));
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(err(format!("path must not contain `..`: {raw}")));
    }
    Ok(normalize(&p))
}

/// Keys accepted by `settings.set`, derived from the defaults so they can never drift.
fn known_keys() -> Vec<String> {
    match serde_json::to_value(Settings::default()) {
        Ok(Value::Object(m)) => m.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

impl Settings {
    /// Validate (and normalize) the top-level `keys`; `None` validates everything.
    pub fn validate(&mut self, env: &Env, keys: Option<&[String]>) -> Result<()> {
        let all = known_keys();
        let todo: Vec<&String> = match keys {
            Some(k) => all.iter().filter(|a| k.contains(a)).collect(),
            None => all.iter().collect(),
        };
        for k in todo {
            self.validate_field(env, k)?;
        }
        Ok(())
    }

    fn validate_field(&mut self, env: &Env, key: &str) -> Result<()> {
        match key {
            "secureDelete" => {
                if !matches!(self.secure_delete.passes, 1 | 3 | 7 | 35) {
                    return Err(err("secureDelete.passes must be one of 1, 3, 7, 35"));
                }
            }
            "tempMinAgeHours" => {
                if self.temp_min_age_hours > 24 * 365 * 10 {
                    return Err(err("tempMinAgeHours is too large"));
                }
            }
            "include" => {
                if self.include.len() > MAX_LIST {
                    return Err(err("too many include entries"));
                }
                let protected = Protected::new(env);
                for e in &mut self.include {
                    if e.id.trim().is_empty() {
                        e.id = random_id();
                    }
                    if e.path.contains(['*', '?']) {
                        return Err(err(format!(
                            "include path `{}` must not contain wildcards; put them in the mask",
                            e.path
                        )));
                    }
                    let p = resolve_user_path(env, &e.path)?;
                    if let Some(why) = protected.include_violation(&p) {
                        return Err(err(format!("`{}` {why}", e.path)));
                    }
                    e.path = e.path.trim().to_string();
                    e.mask = e.mask.trim().to_string();
                    if e.mask.is_empty() {
                        e.mask = default_mask();
                    }
                    if e.mask.contains(['/', '\\']) {
                        return Err(err(format!(
                            "mask `{}` must be a file name pattern, not a path",
                            e.mask
                        )));
                    }
                    globset::Glob::new(&e.mask)
                        .map_err(|x| err(format!("invalid mask `{}`: {x}", e.mask)))?;
                }
                unique_ids("include", self.include.iter().map(|e| &e.id))?;
            }
            "exclude" => {
                if self.exclude.len() > MAX_LIST {
                    return Err(err("too many exclude entries"));
                }
                for e in &mut self.exclude {
                    if e.id.trim().is_empty() {
                        e.id = random_id();
                    }
                    validate_exclude_pattern(env, &e.pattern)?;
                    e.pattern = e.pattern.trim().to_string();
                }
                unique_ids("exclude", self.exclude.iter().map(|e| &e.id))?;
            }
            "cookieKeep" => {
                if self.cookie_keep.len() > 10 * MAX_LIST {
                    return Err(err("too many cookie keep entries"));
                }
                let mut out: Vec<String> = Vec::new();
                for d in &self.cookie_keep {
                    let n =
                        normalize_domain(d).ok_or_else(|| err(format!("invalid domain `{d}`")))?;
                    if !out.contains(&n) {
                        out.push(n);
                    }
                }
                self.cookie_keep = out;
            }
            "selectedRules" => {
                if let Some(v) = &mut self.selected_rules {
                    if v.len() > MAX_LIST {
                        return Err(err("too many selected rules"));
                    }
                    let mut out: Vec<String> = Vec::new();
                    for id in v.iter() {
                        let id = id.trim();
                        if id.is_empty() || id.len() > 128 {
                            return Err(err("invalid rule id in selectedRules"));
                        }
                        if !out.iter().any(|x| x == id) {
                            out.push(id.to_string());
                        }
                    }
                    *v = out;
                }
            }
            "smart" => {
                if !(1..=10_000_000).contains(&self.smart.threshold_mb) {
                    return Err(err("smart.thresholdMb must be between 1 and 10000000"));
                }
                if self.smart.clean_on_browser_close.len() > 100
                    || self
                        .smart
                        .clean_on_browser_close
                        .iter()
                        .any(|s| s.trim().is_empty() || s.len() > 64)
                {
                    return Err(err("invalid smart.cleanOnBrowserClose"));
                }
                let mut seen = HashSet::new();
                self.smart
                    .clean_on_browser_close
                    .retain(|g| seen.insert(g.clone()));
                if !(5..=10_080).contains(&self.smart.check_interval_minutes) {
                    return Err(err(
                        "smart.checkIntervalMinutes must be between 5 and 10080",
                    ));
                }
                if !(1..=1_440).contains(&self.smart.enforce_sleep_minutes) {
                    return Err(err("smart.enforceSleepMinutes must be between 1 and 1440"));
                }
            }
            "ignoredUpdates" => {
                if self.ignored_updates.len() > 10 * MAX_LIST {
                    return Err(err("too many ignored updates"));
                }
                let mut out: Vec<String> = Vec::new();
                for id in &self.ignored_updates {
                    let id = id.trim();
                    if id.is_empty() || id.len() > 300 || id.chars().any(char::is_control) {
                        return Err(err("invalid id in ignoredUpdates"));
                    }
                    if !out.iter().any(|x| x == id) {
                        out.push(id.to_string());
                    }
                }
                self.ignored_updates = out;
            }
            "language" => {
                let l = self.language.trim();
                if !(2..=16).contains(&l.len())
                    || !l
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    return Err(err("language must be a short code like `en` or `pt-BR`"));
                }
                self.language = l.to_string();
            }
            // Enums / booleans are validated by deserialization.
            _ => {}
        }
        Ok(())
    }

    /// Drop anything unsafe/invalid after loading a hand-edited file. Invalid include
    /// entries are dropped (they would delete files); everything else falls back to the
    /// default for that field. Invalid exclusions are kept on purpose: the cleaner
    /// refuses to run with them, which is the safe failure.
    fn sanitize_after_load(&mut self, env: &Env) {
        let d = Settings::default();
        if !matches!(self.secure_delete.passes, 1 | 3 | 7 | 35) {
            self.secure_delete.passes = d.secure_delete.passes;
        }
        if self.temp_min_age_hours > 24 * 365 * 10 {
            self.temp_min_age_hours = d.temp_min_age_hours;
        }
        let includes = std::mem::take(&mut self.include);
        for e in includes {
            let mut probe = Settings {
                include: vec![e.clone()],
                ..Settings::default()
            };
            if probe.validate_field(env, "include").is_ok() {
                self.include.push(probe.include.remove(0));
            }
        }
        let mut ids = HashSet::new();
        self.include.retain(|e| ids.insert(e.id.clone()));
        if self.validate_field(env, "cookieKeep").is_err() {
            self.cookie_keep.retain(|s| normalize_domain(s).is_some());
            let _ = self.validate_field(env, "cookieKeep");
        }
        let _ = self.validate_field(env, "selectedRules");
        if self.validate_field(env, "smart").is_err() {
            self.smart = d.smart;
        }
        if self.validate_field(env, "language").is_err() {
            self.language = d.language;
        }
        if self.validate_field(env, "ignoredUpdates").is_err() {
            self.ignored_updates.retain(|s| {
                let t = s.trim();
                !t.is_empty() && t.len() <= 300 && !t.chars().any(char::is_control)
            });
            self.ignored_updates.truncate(10 * MAX_LIST);
            let _ = self.validate_field(env, "ignoredUpdates");
        }
        // Exclusions need ids, but stay untouched otherwise.
        let mut seen = HashSet::new();
        for e in &mut self.exclude {
            if e.id.trim().is_empty() || !seen.insert(e.id.clone()) {
                e.id = random_id();
                seen.insert(e.id.clone());
            }
        }
    }
}

// ---------------------------------------------------------------- persistence

static LOCK: Mutex<()> = Mutex::new(());

fn settings_path(env: &Env) -> PathBuf {
    env.data_dir.join("settings.json")
}

/// Deep merge per RFC 7386 (`null` deletes a key, arrays/scalars replace).
pub fn merge_patch(target: &mut Value, patch: &Value) {
    if let Value::Object(p) = patch {
        if !target.is_object() {
            *target = Value::Object(Map::new());
        }
        if let Value::Object(t) = target {
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
    } else {
        *target = patch.clone();
    }
}

fn defaults_value() -> Value {
    serde_json::to_value(Settings::default()).unwrap_or(Value::Null)
}

/// Build settings from arbitrary JSON, dropping (rather than failing on) any top-level
/// field that does not deserialize. Returns the settings and whether anything was dropped.
fn lenient_from_value(v: &Value) -> (Settings, bool) {
    let Value::Object(file) = v else {
        return (Settings::default(), true);
    };
    let mut acc = match defaults_value() {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let mut dropped = false;
    for (k, val) in file {
        if !known_keys().contains(k) {
            continue; // unknown keys (newer version, or junk) are ignored
        }
        let mut candidate = acc.clone();
        let mut single = Value::Object(std::mem::take(&mut candidate));
        merge_patch(
            &mut single,
            &Value::Object(Map::from_iter([(k.clone(), val.clone())])),
        );
        if serde_json::from_value::<Settings>(single.clone()).is_ok() {
            if let Value::Object(m) = single {
                acc = m;
            }
        } else {
            dropped = true;
        }
    }
    let s = serde_json::from_value::<Settings>(Value::Object(acc)).unwrap_or_default();
    (s, dropped)
}

fn backup_corrupt(env: &Env) {
    let path = settings_path(env);
    let ts = now_unix();
    for n in 0..100u32 {
        let name = if n == 0 {
            format!("settings.json.corrupt-{ts}")
        } else {
            format!("settings.json.corrupt-{ts}-{n}")
        };
        let dest = env.data_dir.join(name);
        if !dest.exists() {
            let _ = fs::rename(&path, dest);
            return;
        }
    }
}

fn load_locked(env: &Env) -> Settings {
    let path = settings_path(env);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Settings::default(),
    };
    let parsed: std::result::Result<Value, _> = serde_json::from_slice(&bytes);
    match parsed {
        Ok(v @ Value::Object(_)) => {
            let (mut s, dropped) = lenient_from_value(&v);
            if dropped {
                // Keep the original around so nothing the user wrote is lost silently.
                let _ = fs::copy(
                    &path,
                    env.data_dir
                        .join(format!("settings.json.corrupt-{}", now_unix())),
                );
            }
            s.sanitize_after_load(env);
            s
        }
        _ => {
            backup_corrupt(env);
            Settings::default()
        }
    }
}

fn save_locked(env: &Env, s: &Settings) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(s)?;
    atomic_write(&settings_path(env), &bytes)?;
    Ok(())
}

/// Load the settings; never fails (corrupt files are backed up and defaults returned).
pub fn load(ctx: &Ctx) -> Settings {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load_locked(&ctx.env)
}

/// Atomically load, modify, validate and save.
pub fn update(ctx: &Ctx, f: impl FnOnce(&mut Settings)) -> Result<Settings> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = load_locked(&ctx.env);
    f(&mut s);
    s.validate(&ctx.env, None)?;
    save_locked(&ctx.env, &s)?;
    Ok(s)
}

// ---------------------------------------------------------------- API

/// `runAtStartup` as the OS reports it: the autostart entry is the truth, the saved value is
/// only a copy of it (someone may have removed the entry by hand or through the startup
/// manager).
fn reconcile_startup(ctx: &Ctx, s: &mut Settings) {
    if let Some(actual) = crate::autostart::is_installed(ctx) {
        s.run_at_startup = actual;
    }
}

fn get(ctx: &Ctx, _params: Value, _job: &Job) -> Result<Value> {
    let mut s = load(ctx);
    reconcile_startup(ctx, &mut s);
    Ok(serde_json::to_value(s)?)
}

fn set(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let Value::Object(patch) = &params else {
        return Err(err("settings.set expects a JSON object"));
    };
    let known = known_keys();
    if let Some(bad) = patch.keys().find(|k| !known.contains(k)) {
        return Err(err(format!("unknown setting `{bad}`")));
    }
    let touched: Vec<String> = patch.keys().cloned().collect();
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let current = load_locked(&ctx.env);
    let mut merged = serde_json::to_value(&current)?;
    merge_patch(&mut merged, &params);
    let mut next: Settings =
        serde_json::from_value(merged).map_err(|e| err(format!("invalid settings: {e}")))?;
    next.validate(&ctx.env, Some(&touched))?;
    if touched.iter().any(|k| k == "runAtStartup") {
        // Change the OS first: when that fails nothing is saved and the caller sees why.
        crate::autostart::apply(ctx, next.run_at_startup)?;
        if let Err(e) = save_locked(&ctx.env, &next) {
            let _ = crate::autostart::apply(ctx, current.run_at_startup);
            return Err(e);
        }
    } else {
        // Never persist a stale copy of the startup flag along with an unrelated change.
        reconcile_startup(ctx, &mut next);
        save_locked(&ctx.env, &next)?;
    }
    Ok(serde_json::to_value(next)?)
}

fn reset(ctx: &Ctx, _params: Value, _job: &Job) -> Result<Value> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = Settings::default();
    // Defaults mean "no autostart entry".
    crate::autostart::apply(ctx, false)?;
    save_locked(&ctx.env, &s)?;
    Ok(serde_json::to_value(s)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dispatch;
    use crate::error::ErrorCode;
    use crate::runner::MockRunner;
    use serde_json::json;

    fn ctx() -> (tempfile::TempDir, Ctx) {
        let d = tempfile::tempdir().unwrap();
        let c = Ctx::test(d.path(), MockRunner::new());
        fs::create_dir_all(&c.env.home).unwrap();
        (d, c)
    }

    fn call(c: &Ctx, m: &str, p: Value) -> Result<Value> {
        dispatch(c, m, p, &Job::detached())
    }

    #[test]
    fn defaults_match_spec_and_use_camel_case() {
        let (_d, c) = ctx();
        let v = call(&c, "settings.get", Value::Null).unwrap();
        assert_eq!(
            v,
            json!({
                "theme": "system",
                "secureDelete": {"enabled": false, "passes": 1},
                "closeBrowsers": "ask",
                "tempMinAgeHours": 24,
                "include": [],
                "exclude": [],
                "cookieKeep": [],
                "selectedRules": null,
                "smart": {"enabled": false, "thresholdMb": 500, "cleanOnBrowserClose": [],
                          "autoClean": false, "notify": true,
                          "checkIntervalMinutes": 60, "enforceSleepMinutes": 15},
                "runAtStartup": false,
                "closeToTray": true,
                "language": "en",
                "ignoredUpdates": []
            })
        );
        // get does not create the file
        assert!(!c.env.data_dir.join("settings.json").exists());
    }

    #[test]
    fn set_deep_merges_and_persists() {
        let (_d, c) = ctx();
        let v = call(
            &c,
            "settings.set",
            json!({"theme": "dark", "secureDelete": {"enabled": true}}),
        )
        .unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["secureDelete"], json!({"enabled": true, "passes": 1}));
        let v = call(&c, "settings.set", json!({"secureDelete": {"passes": 7}})).unwrap();
        assert_eq!(v["secureDelete"], json!({"enabled": true, "passes": 7}));
        let again = load(&c);
        assert_eq!(again.theme, Theme::Dark);
        assert_eq!(again.secure_delete.passes, 7);
        assert!(again.secure_delete.enabled);
        // null deletes a key => back to its default
        let v = call(&c, "settings.set", json!({"theme": null})).unwrap();
        assert_eq!(v["theme"], "system");
    }

    #[test]
    fn set_replaces_arrays_and_supports_selected_rules_null() {
        let (_d, c) = ctx();
        call(
            &c,
            "settings.set",
            json!({"selectedRules": ["a.b", "c.d", "a.b"]}),
        )
        .unwrap();
        assert_eq!(
            load(&c).selected_rules,
            Some(vec!["a.b".into(), "c.d".into()])
        );
        call(&c, "settings.set", json!({"selectedRules": ["x.y"]})).unwrap();
        assert_eq!(load(&c).selected_rules, Some(vec!["x.y".into()]));
        call(&c, "settings.set", json!({"selectedRules": null})).unwrap();
        assert_eq!(load(&c).selected_rules, None);
    }

    #[test]
    fn set_validation_errors_and_no_partial_write() {
        let (_d, c) = ctx();
        call(&c, "settings.set", json!({"theme": "dark"})).unwrap();
        for bad in [
            json!({"secureDelete": {"passes": 5}}),
            json!({"secureDelete": {"passes": 0}}),
            json!({"theme": "purple"}),
            json!({"closeBrowsers": "maybe"}),
            json!({"tempMinAgeHours": -1}),
            json!({"language": ""}),
            json!({"nonsense": true}),
            json!({"smart": {"thresholdMb": 0}}),
            json!([1, 2]),
            json!({"cookieKeep": ["bad domain with spaces"]}),
        ] {
            let e = call(&c, "settings.set", bad.clone()).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidParams, "{bad}");
        }
        // The failed calls changed nothing.
        let s = load(&c);
        assert_eq!(s.theme, Theme::Dark);
        assert_eq!(s.secure_delete.passes, 1);
    }

    #[test]
    fn include_and_exclude_validation() {
        let (_d, c) = ctx();
        let home = c.env.home.to_string_lossy().into_owned();
        // ok: ~ expansion, defaults filled, id generated
        let v = call(
            &c,
            "settings.set",
            json!({"include": [{"path": "~/scratch", "recursive": true, "mask": "*.tmp"}],
                   "exclude": [{"id": "e", "pattern": "~/scratch/keep*"}]}),
        )
        .unwrap();
        assert!(!v["include"][0]["id"].as_str().unwrap().is_empty());
        assert_eq!(v["include"][0]["mask"], "*.tmp");
        // missing mask => "*"
        let v = call(
            &c,
            "settings.set",
            json!({"include": [{"path": format!("{home}/x")}]}),
        )
        .unwrap();
        assert_eq!(v["include"][0]["mask"], "*");
        assert_eq!(v["include"][0]["recursive"], false);

        for bad in [
            json!({"include": [{"path": "relative"}]}),
            json!({"include": [{"path": "~"}]}),
            json!({"include": [{"path": "/"}]}),
            json!({"include": [{"path": "~/Documents"}]}),
            json!({"include": [{"path": "~/.ssh"}]}),
            json!({"include": [{"path": "~/.ssh/keys"}]}), // inside .ssh
            json!({"include": [{"path": c.env.sys_path("/etc")}]}),
            json!({"include": [{"path": c.env.sys_path("/usr/local/share")}]}),
            json!({"include": [{"path": "~/.config"}]}),
            json!({"include": [{"path": "~/.local/share"}]}),
            json!({"include": [{"path": "~/a/../.."}]}),
            json!({"include": [{"path": "~/a/*"}]}),
            json!({"include": [{"path": "~/a", "mask": "a/b"}]}),
            json!({"include": [{"path": "~/a", "mask": "[oops"}]}),
            json!({"include": [{"id": "d", "path": "~/a"}, {"id": "d", "path": "~/b"}]}),
            json!({"exclude": [{"pattern": "relative/glob"}]}),
            json!({"exclude": [{"pattern": ""}]}),
            json!({"exclude": [{"pattern": "/a/../b"}]}),
            json!({"exclude": [{"id": "d", "pattern": "/a"}, {"id": "d", "pattern": "/b"}]}),
        ] {
            let e = call(&c, "settings.set", bad.clone());
            assert!(e.is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn cookie_keep_is_normalized_and_deduped() {
        let (_d, c) = ctx();
        let v = call(
            &c,
            "settings.set",
            json!({"cookieKeep": [" .Google.com ", "google.com", "GitHub.com"]}),
        )
        .unwrap();
        assert_eq!(v["cookieKeep"], json!(["google.com", "github.com"]));
    }

    #[test]
    fn reset_restores_defaults() {
        let (_d, c) = ctx();
        call(
            &c,
            "settings.set",
            json!({"theme": "light", "runAtStartup": true}),
        )
        .unwrap();
        let v = call(&c, "settings.reset", Value::Null).unwrap();
        assert_eq!(v, serde_json::to_value(Settings::default()).unwrap());
        assert_eq!(load(&c), Settings::default());
    }

    #[test]
    fn missing_and_unknown_fields_do_not_break_loading() {
        let (_d, c) = ctx();
        fs::create_dir_all(&c.env.data_dir).unwrap();
        fs::write(
            c.env.data_dir.join("settings.json"),
            r#"{"theme":"dark","futureThing":{"x":1},"smart":{"enabled":true}}"#,
        )
        .unwrap();
        let s = load(&c);
        assert_eq!(s.theme, Theme::Dark);
        assert!(s.smart.enabled);
        assert_eq!(s.smart.threshold_mb, 500);
        assert_eq!(s.language, "en");
        // no backup was made for a merely-old file
        let backups = fs::read_dir(&c.env.data_dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("corrupt")
            })
            .count();
        assert_eq!(backups, 0);
    }

    #[test]
    fn corrupt_file_is_backed_up_and_defaults_used() {
        let (_d, c) = ctx();
        fs::create_dir_all(&c.env.data_dir).unwrap();
        fs::write(c.env.data_dir.join("settings.json"), "{ this is not json").unwrap();
        let s = load(&c);
        assert_eq!(s, Settings::default());
        let names: Vec<String> = fs::read_dir(&c.env.data_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("settings.json.corrupt-")),
            "{names:?}"
        );
        assert!(!names.contains(&"settings.json".to_string()));
        // and it recovers: saving works afterwards
        call(&c, "settings.set", json!({"theme": "dark"})).unwrap();
        assert_eq!(load(&c).theme, Theme::Dark);
    }

    #[test]
    fn wrong_typed_field_is_dropped_but_others_survive_with_backup() {
        let (_d, c) = ctx();
        fs::create_dir_all(&c.env.data_dir).unwrap();
        fs::write(
            c.env.data_dir.join("settings.json"),
            r#"{"theme": 42, "tempMinAgeHours": 48}"#,
        )
        .unwrap();
        let s = load(&c);
        assert_eq!(s.theme, Theme::System);
        assert_eq!(s.temp_min_age_hours, 48);
        assert!(fs::read_dir(&c.env.data_dir).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("corrupt")));
    }

    #[test]
    fn hand_edited_unsafe_include_is_dropped_on_load() {
        let (_d, c) = ctx();
        fs::create_dir_all(&c.env.data_dir).unwrap();
        fs::write(
            c.env.data_dir.join("settings.json"),
            format!(
                r#"{{"include":[{{"id":"a","path":"{h}"}},{{"id":"b","path":"{h}/ok"}}],
                    "secureDelete":{{"passes":5}}}}"#,
                h = c.env.home.display()
            ),
        )
        .unwrap();
        let s = load(&c);
        assert_eq!(s.include.len(), 1);
        assert_eq!(s.include[0].id, "b");
        assert_eq!(s.secure_delete.passes, 1);
    }

    #[test]
    fn set_only_validates_touched_fields() {
        let (_d, c) = ctx();
        fs::create_dir_all(&c.env.data_dir).unwrap();
        // an invalid exclusion in the file must not block unrelated edits...
        fs::write(
            c.env.data_dir.join("settings.json"),
            r#"{"exclude":[{"id":"x","pattern":"not-absolute"}]}"#,
        )
        .unwrap();
        call(&c, "settings.set", json!({"theme": "dark"})).unwrap();
        // ...and it is kept (so the cleaner refuses to run) until the user fixes it.
        assert_eq!(load(&c).exclude.len(), 1);
        call(&c, "settings.set", json!({"exclude": []})).unwrap();
        assert!(load(&c).exclude.is_empty());
    }

    #[test]
    fn concurrent_updates_do_not_lose_writes() {
        let (_d, c) = ctx();
        let c = std::sync::Arc::new(c);
        let mut hs = Vec::new();
        for i in 0..8 {
            let c = c.clone();
            hs.push(std::thread::spawn(move || {
                update(&c, |s| s.cookie_keep.push(format!("site{i}.com"))).unwrap();
            }));
        }
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(load(&c).cookie_keep.len(), 8);
    }

    #[test]
    fn update_helper_validates() {
        let (_d, c) = ctx();
        assert!(update(&c, |s| s.secure_delete.passes = 4).is_err());
        assert_eq!(load(&c).secure_delete.passes, 1);
        let s = update(&c, |s| s.theme = Theme::Light).unwrap();
        assert_eq!(s.theme, Theme::Light);
    }

    #[test]
    fn merge_patch_rfc7386_examples() {
        let mut t = json!({"a": "b", "c": {"d": "e", "f": "g"}});
        merge_patch(&mut t, &json!({"a": "z", "c": {"f": null}}));
        assert_eq!(t, json!({"a": "z", "c": {"d": "e"}}));
        let mut t = json!({"a": [{"b": "c"}]});
        merge_patch(&mut t, &json!({"a": [1]}));
        assert_eq!(t, json!({"a": [1]}));
        let mut t = json!({"a": "foo"});
        merge_patch(&mut t, &json!("bar"));
        assert_eq!(t, json!("bar"));
    }
}
