//! Smart cleaning: settings + junk measurement for the background agent (`crate::agent`).
//!
//! Methods:
//! - `smart_cleaning.get_config`: the `smart` settings (`enabled`, `thresholdMb`, `notify`,
//!   `autoClean`, `cleanOnBrowserClose`, `checkIntervalMinutes`, `enforceSleepMinutes`).
//! - `smart_cleaning.set_config { ...partial smart settings }`: validated merge; returns the
//!   new config. `cleanOnBrowserClose` entries must be browser groups known on this system.
//! - `smart_cleaning.check`: measure the junk of the user's enabled rules now:
//!   `{junkBytes, thresholdBytes, overThreshold, cleaned?}`. When smart cleaning and
//!   `autoClean` are on and the junk is over the threshold it is cleaned right away
//!   (history source `smart`) and `cleaned` describes the result.
//! - `smart_cleaning.status`: the background agent's status from `agent-status.json`:
//!   `{running, pid?, since?, lastCheck?, lastJunkBytes?, lastAction?, lastActionAt?}`.
//!   `running` is verified against the agent's lock file, not trusted from the file.
//!
//! Run at startup lives in `settings.set { runAtStartup }` (see `crate::autostart`).

use serde::Serialize;
use serde_json::{Map, Value};

use crate::api::Registry;
use crate::ctx::Ctx;
use crate::error::{ApiError, Result};
use crate::features::cleaner::{self, rules::Category, CleanOptions, Source};
use crate::features::settings::{self, Settings, SmartSettings};
use crate::job::Job;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "smart_cleaning.get_config",
    "smart_cleaning.set_config",
    "smart_cleaning.check",
    "smart_cleaning.status",
];

pub fn register(r: &mut Registry) {
    r.add("smart_cleaning.get_config", get_config);
    r.add("smart_cleaning.set_config", set_config);
    r.add("smart_cleaning.check", check);
    r.add("smart_cleaning.status", status);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cleaned {
    pub removed_bytes: u64,
    pub removed_files: u64,
    pub removed_rows: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub junk_bytes: u64,
    pub threshold_bytes: u64,
    pub over_threshold: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleaned: Option<Cleaned>,
    /// Why the automatic clean did not happen although it was due.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clean_error: Option<String>,
}

/// Measure the junk of the enabled rules; auto-clean it when configured and over the threshold.
pub fn check_junk(ctx: &Ctx, s: &Settings, job: &Job) -> Result<CheckResult> {
    let report = cleaner::analyze(ctx, None, job)?;
    let threshold_bytes = u64::from(s.smart.threshold_mb) * 1024 * 1024;
    let over = report.total_bytes >= threshold_bytes;
    let mut out = CheckResult {
        junk_bytes: report.total_bytes,
        threshold_bytes,
        over_threshold: over,
        cleaned: None,
        clean_error: None,
    };
    if over && s.smart.enabled && s.smart.auto_clean {
        let opts = CleanOptions::from_settings(s, true);
        match cleaner::run_clean(ctx, None, &opts, Source::Smart, job) {
            Ok(r) => {
                out.cleaned = Some(Cleaned {
                    removed_bytes: r.total_bytes,
                    removed_files: r.total_files,
                    removed_rows: r.total_rows,
                    history_id: r.history_id,
                })
            }
            Err(e) if e.code == crate::error::ErrorCode::Cancelled => return Err(e),
            Err(e) => out.clean_error = Some(e.message),
        }
    }
    Ok(out)
}

fn get_config(ctx: &Ctx, _p: Value, _job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(settings::load(ctx).smart)?)
}

fn set_config(ctx: &Ctx, p: Value, _job: &Job) -> Result<Value> {
    let Value::Object(patch) = p else {
        return Err(ApiError::invalid_params(
            "smart_cleaning.set_config expects a JSON object",
        ));
    };
    let known: Vec<String> = match serde_json::to_value(SmartSettings::default())? {
        Value::Object(m) => m.keys().cloned().collect(),
        _ => Vec::new(),
    };
    if let Some(bad) = patch.keys().find(|k| !known.contains(k)) {
        return Err(ApiError::invalid_params(format!(
            "unknown smart cleaning setting `{bad}`"
        )));
    }
    let current = settings::load(ctx);
    if let Some(Value::Array(groups)) = patch.get("cleanOnBrowserClose") {
        let browsers = browser_groups(ctx, &current);
        for g in groups {
            let name = g.as_str().unwrap_or_default();
            if !browsers.iter().any(|b| b == name) {
                return Err(ApiError::invalid_params(format!(
                    "`{name}` is not a browser ClearSweep can clean on this system"
                )));
            }
        }
    }
    let mut merged = serde_json::to_value(&current.smart)?;
    settings::merge_patch(&mut merged, &Value::Object(Map::from_iter(patch)));
    let next: SmartSettings = serde_json::from_value(merged)
        .map_err(|e| ApiError::invalid_params(format!("invalid smart cleaning settings: {e}")))?;
    let saved = settings::update(ctx, |s| s.smart = next)?;
    Ok(serde_json::to_value(saved.smart)?)
}

/// Browser groups (`Google Chrome`, ...) that have rules on this system.
pub fn browser_groups(ctx: &Ctx, s: &Settings) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for r in cleaner::available_rules(ctx, s) {
        if r.category == Category::Browser && !v.contains(&r.group) {
            v.push(r.group);
        }
    }
    v
}

fn check(ctx: &Ctx, _p: Value, job: &Job) -> Result<Value> {
    let s = settings::load(ctx);
    Ok(serde_json::to_value(check_junk(ctx, &s, job)?)?)
}

fn status(ctx: &Ctx, _p: Value, _job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(crate::agent::read_status(ctx))?)
}

#[cfg(test)]
mod tests;
