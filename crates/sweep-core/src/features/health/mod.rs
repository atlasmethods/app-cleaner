//! Health Check: one-click overview of junk, privacy, speed and security issues, and a fix
//! for exactly what the user reviewed.
//!
//! Methods:
//! - `health.analyze`: cancellable; progress per category. Returns
//!   `{ score, scannedAt, categories: [{ id, title, status, summary, findings, fixable, metrics }] }`
//!   for the four categories `privacy`, `space`, `speed` and `security`. Each is computed on
//!   its own: a category that fails is `unavailable` (the reason is its `summary`) and the
//!   rest are still reported. The result is also stored as the last result.
//! - `health.last`: the stored last result, or `null` when there is none.
//! - `health.fix { privacy?, space?, startupIds?, sleepAppIds?, updateIds?, closeApps? }`:
//!   does ONLY what is listed, part by part (cancellable between parts), and returns the
//!   per-part results plus a fresh analysis. `closeApps` (`skip` by default) follows the
//!   cleaner's semantics: a running browser is left alone and reported in `blockedApps`.
//!
//! What each part means:
//! - `privacy`: the cleaner over the enabled browser cookie / history / download rules
//!   (the cookie keep list is honoured);
//! - `space`: the cleaner over the other enabled rules (the Clean tab's selection);
//! - `startupIds`: `startup` switch-off; every id is looked up again and refused unless it is
//!   a user-level, non-critical autostart item;
//! - `sleepAppIds`: optimizer sleep, for background apps only;
//! - `updateIds`: software updater, for ids that really have an update and are not ignored.
//!
//! The score formula is documented in [`score`].

use serde::Deserialize;
use serde_json::Value;

use crate::api::Registry;
use crate::ctx::Ctx;
use crate::error::{ApiError, Result};
use crate::features::settings::CloseBrowsers;
use crate::job::Job;

pub mod analyze;
pub mod fix;
pub mod model;
pub mod score;
pub mod store;

#[cfg(test)]
mod tests;

pub use fix::FixRequest;
pub use model::{FixReport, HealthReport};

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["health.analyze", "health.last", "health.fix"];

pub fn register(r: &mut Registry) {
    r.add("health.analyze", analyze_handler);
    r.add("health.last", last_handler);
    r.add("health.fix", fix_handler);
}

/// Scan, store the result as the last one, and return it.
pub fn run_analyze(ctx: &Ctx, job: &Job) -> Result<HealthReport> {
    let report = analyze::analyze(ctx, job)?;
    // Failing to remember the result must not fail the scan.
    let _ = store::save(ctx, &report);
    Ok(report)
}

fn analyze_handler(ctx: &Ctx, _params: Value, job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(run_analyze(ctx, job)?)?)
}

fn last_handler(ctx: &Ctx, _params: Value, _job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(store::load(ctx))?)
}

const MAX_IDS: usize = 500;
const MAX_ID_LEN: usize = 300;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
struct FixParams {
    privacy: bool,
    space: bool,
    startup_ids: Vec<String>,
    sleep_app_ids: Vec<String>,
    update_ids: Vec<String>,
    close_apps: Option<CloseBrowsers>,
}

fn check_ids(name: &str, ids: &[String]) -> Result<()> {
    if ids.len() > MAX_IDS || ids.iter().any(|i| i.is_empty() || i.len() > MAX_ID_LEN) {
        return Err(ApiError::invalid_params(format!("invalid `{name}`")));
    }
    Ok(())
}

fn fix_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: FixParams = if params.is_null() {
        FixParams::default()
    } else {
        serde_json::from_value(params)?
    };
    check_ids("startupIds", &p.startup_ids)?;
    check_ids("sleepAppIds", &p.sleep_app_ids)?;
    check_ids("updateIds", &p.update_ids)?;
    let req = FixRequest {
        privacy: p.privacy,
        space: p.space,
        startup_ids: p.startup_ids,
        sleep_app_ids: p.sleep_app_ids,
        update_ids: p.update_ids,
        close_apps: p.close_apps.unwrap_or(CloseBrowsers::Skip),
    };
    Ok(serde_json::to_value(fix::fix(ctx, &req, job)?)?)
}
