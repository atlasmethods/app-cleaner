//! Cleaner: rule-driven analysis and removal of temp files, caches, logs and browser data.
//!
//! Methods:
//! - `cleaner.list_rules`: rules for this OS grouped by category and group.
//! - `cleaner.analyze { ruleIds? }`: read-only scan; reports what a clean would remove.
//! - `cleaner.clean { ruleIds?, closeApps? }`: re-scans on the server side (paths are
//!   never accepted from the client) and deletes through `SafeDeleter`.
//! - `cleaner.history { limit? }`: past runs.
//!
//! Rules live in `rules/*.toml` (documented in `rules/README.md`).

use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use crate::api::Registry;
use crate::ctx::Ctx;
use crate::error::{ApiError, ErrorCode, Result};
use crate::fsutil::{now_rfc3339, random_id};
use crate::job::{Job, ProgressEvent};
use crate::procs::ProcInfo;
use crate::safety::{ExcludeSet, Safety};

use crate::features::settings::{self, CloseBrowsers, Settings};

pub mod apps;
pub mod engine;
pub mod firefox;
pub mod history;
pub mod model;
#[cfg(windows)]
pub mod registry_win;
pub mod rules;
pub mod sqlite;
pub mod template;

use engine::{Engine, Mode, Outcome};
pub use model::{
    AnalyzeItem, AnalyzeReport, CleanOptions, CleanReport, HistoryEntry, RuleClean, Skipped, Source,
};
use model::{CategoryInfo, GroupInfo, RuleInfo, RulesListing};
use rules::{Category, FilesTarget, OsName, Rule, Target};

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "cleaner.list_rules",
    "cleaner.analyze",
    "cleaner.clean",
    "cleaner.history",
];

pub fn register(r: &mut Registry) {
    r.add("cleaner.list_rules", list_rules_handler);
    r.add("cleaner.analyze", analyze_handler);
    r.add("cleaner.clean", clean_handler);
    r.add("cleaner.history", history_handler);
}

pub const CUSTOM_RULE_ID: &str = "custom.include";

// ---------------------------------------------------------------- rule selection

/// The synthetic rule built from the user's include entries.
pub fn custom_rule(ctx: &Ctx, s: &Settings) -> Option<Rule> {
    let protected = crate::safety::Protected::new(&ctx.env);
    let targets: Vec<Target> = s
        .include
        .iter()
        .filter_map(|e| {
            let base = settings::resolve_user_path(&ctx.env, &e.path).ok()?;
            // Defense in depth: settings validation already refuses these.
            protected.include_violation(&base).is_none().then_some(())?;
            Some(Target::Files(FilesTarget {
                os: Vec::new(),
                base: String::new(),
                literal_base: Some(base),
                pattern: e.mask.clone(),
                recursive: e.recursive,
                min_age_hours: None,
                min_age_from_settings: false,
                remove_empty_dirs: e.remove_empty_dirs,
                keep_base: true,
                owned_by_user: false,
                skip_names: Vec::new(),
            }))
        })
        .collect();
    if targets.is_empty() {
        return None;
    }
    Some(Rule {
        id: CUSTOM_RULE_ID.to_string(),
        name: "Custom folders".to_string(),
        group: "Custom".to_string(),
        category: Category::System,
        os: vec![OsName::Linux, OsName::Windows, OsName::Macos],
        default_enabled: true,
        description: "Files in the folders you added under Settings > Include.".to_string(),
        warning: None,
        processes: Default::default(),
        targets,
    })
}

/// All rules available on this OS, plus the custom include rule when configured.
pub fn available_rules(ctx: &Ctx, s: &Settings) -> Vec<Rule> {
    let mut v: Vec<Rule> = rules::rules_for_os(ctx.env.os)
        .into_iter()
        .cloned()
        .collect();
    v.extend(custom_rule(ctx, s));
    v
}

fn is_enabled(rule: &Rule, s: &Settings) -> bool {
    match &s.selected_rules {
        Some(sel) => sel.contains(&rule.id),
        None => rule.default_enabled,
    }
}

/// Explicit ids (validated) or, when `None`, the rules enabled in settings.
pub fn select_rules(ctx: &Ctx, s: &Settings, ids: Option<Vec<String>>) -> Result<Vec<Rule>> {
    let all = available_rules(ctx, s);
    match ids {
        None => Ok(all.into_iter().filter(|r| is_enabled(r, s)).collect()),
        Some(ids) => {
            let mut seen = HashSet::new();
            let mut out = Vec::new();
            for id in ids {
                if !seen.insert(id.clone()) {
                    continue;
                }
                let rule = all.iter().find(|r| r.id == id).ok_or_else(|| {
                    ApiError::invalid_params(format!("unknown rule `{id}` on this system"))
                })?;
                out.push(rule.clone());
            }
            Ok(out)
        }
    }
}

// ---------------------------------------------------------------- list_rules

fn category_label(c: Category) -> &'static str {
    match c {
        Category::Browser => "Browsers",
        Category::System => "System",
        Category::Application => "Applications",
    }
}

pub fn list_rules(ctx: &Ctx) -> RulesListing {
    let s = settings::load(ctx);
    let all = available_rules(ctx, &s);
    let mut categories = Vec::new();
    for cat in [Category::Browser, Category::System, Category::Application] {
        let mut groups: Vec<GroupInfo> = Vec::new();
        for r in all.iter().filter(|r| r.category == cat) {
            let info = RuleInfo {
                id: r.id.clone(),
                name: r.name.clone(),
                description: r.description.clone(),
                warning: r.warning.clone(),
                enabled: is_enabled(r, &s),
                default_enabled: r.default_enabled,
            };
            match groups.iter_mut().find(|g| g.group == r.group) {
                Some(g) => g.rules.push(info),
                None => groups.push(GroupInfo {
                    group: r.group.clone(),
                    rules: vec![info],
                }),
            }
        }
        if !groups.is_empty() {
            categories.push(CategoryInfo {
                category: cat,
                label: category_label(cat).to_string(),
                groups,
            });
        }
    }
    RulesListing { categories }
}

// ---------------------------------------------------------------- analyze

fn build_safety(ctx: &Ctx, s: &Settings) -> Result<Arc<Safety>> {
    Ok(Arc::new(Safety::new(
        &ctx.env,
        ExcludeSet::from_settings(&ctx.env, s)?,
    )))
}

fn engine<'a>(ctx: &'a Ctx, s: &'a Settings, safety: Arc<Safety>, mode: Mode) -> Engine<'a> {
    Engine {
        ctx,
        settings: s,
        safety,
        keep: s.cookie_keep.clone(),
        now: SystemTime::now(),
        mode,
        secure_passes: (mode == Mode::Clean && s.secure_delete.enabled)
            .then_some(s.secure_delete.passes),
    }
}

/// Read-only scan of the given (or all enabled) rules. Modifies nothing.
pub fn analyze(ctx: &Ctx, rule_ids: Option<Vec<String>>, job: &Job) -> Result<AnalyzeReport> {
    let start = Instant::now();
    let s = settings::load(ctx);
    let rules = select_rules(ctx, &s, rule_ids)?;
    let safety = build_safety(ctx, &s)?;
    let eng = engine(ctx, &s, safety, Mode::Analyze);
    let procs = ctx.procs.list();

    let total = rules.len() as u64;
    let mut items = Vec::with_capacity(rules.len());
    let (mut tf, mut tb, mut tr) = (0u64, 0u64, 0u64);
    for (i, rule) in rules.iter().enumerate() {
        job.check_cancelled()?;
        job.progress(
            ProgressEvent::new("analyze")
                .fraction(i as f64 / total.max(1) as f64)
                .counts(i as u64, total)
                .message(format!("{} - {}", rule.group, rule.name)),
        );
        let mut out = Outcome::default();
        eng.run_rule(rule, job, &mut out)?;
        tf += out.files;
        tb += out.bytes;
        tr += out.rows;
        items.push(AnalyzeItem {
            rule_id: rule.id.clone(),
            name: rule.name.clone(),
            group: rule.group.clone(),
            category: rule.category,
            files: out.files,
            bytes: out.bytes,
            rows: out.rows,
            sample_paths: out.samples,
            app_running: apps::is_running(rule, ctx.env.os, &procs),
            errors: out.errors,
            actions: out.actions,
        });
    }
    job.progress(
        ProgressEvent::new("analyze")
            .fraction(1.0)
            .counts(total, total),
    );
    Ok(AnalyzeReport {
        items,
        total_files: tf,
        total_bytes: tb,
        total_rows: tr,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

// ---------------------------------------------------------------- clean

fn skipped_result(rule: &Rule, why: Skipped, running: Vec<String>) -> RuleClean {
    RuleClean {
        rule_id: rule.id.clone(),
        removed_files: 0,
        removed_bytes: 0,
        removed_rows: 0,
        failed: Vec::new(),
        skipped: Some(why),
        actions: Vec::new(),
        running_apps: running,
        closed_apps: Vec::new(),
    }
}

/// Clean the given (or all enabled) rules.
///
/// Paths are always discovered here, on the server side, by re-scanning; nothing that
/// identifies files to delete is ever taken from a caller. Used by the API, the CLI and
/// (later) the scheduler, smart cleaning and the health check.
pub fn run_clean(
    ctx: &Ctx,
    rule_ids: Option<Vec<String>>,
    opts: &CleanOptions,
    source: Source,
    job: &Job,
) -> Result<CleanReport> {
    let start = Instant::now();
    let s = settings::load(ctx);
    let rules = select_rules(ctx, &s, rule_ids)?;
    let safety = build_safety(ctx, &s)?;
    let eng = engine(ctx, &s, safety, Mode::Clean);
    let mut procs: Vec<ProcInfo> = ctx.procs.list();

    let total = rules.len() as u64;
    let mut results: Vec<RuleClean> = Vec::with_capacity(rules.len());
    let mut ran_ids: Vec<String> = Vec::new();
    let (mut tf, mut tb, mut tr) = (0u64, 0u64, 0u64);
    let mut cancelled = false;

    for (i, rule) in rules.iter().enumerate() {
        if job.is_cancelled() {
            cancelled = true;
            break;
        }
        job.progress(
            ProgressEvent::new("clean")
                .fraction(i as f64 / total.max(1) as f64)
                .counts(i as u64, total)
                .message(format!("{} - {}", rule.group, rule.name)),
        );

        let mut closed_apps = Vec::new();
        if apps::is_running(rule, ctx.env.os, &procs) {
            match opts.close_apps {
                CloseBrowsers::Ask | CloseBrowsers::Skip => {
                    results.push(skipped_result(
                        rule,
                        Skipped::AppRunning,
                        vec![rule.group.clone()],
                    ));
                    continue;
                }
                CloseBrowsers::Always => {
                    let (fresh, gone) =
                        apps::close_and_wait(ctx, rule, &procs, opts.close_timeout, job);
                    procs = fresh;
                    if !gone {
                        results.push(skipped_result(
                            rule,
                            Skipped::AppRunning,
                            vec![rule.group.clone()],
                        ));
                        continue;
                    }
                    closed_apps.push(rule.group.clone());
                }
            }
        }

        let mut out = Outcome::default();
        let run = eng.run_rule(rule, job, &mut out);
        let was_cancelled = matches!(&run, Err(e) if e.code == ErrorCode::Cancelled);
        if let Err(e) = run {
            if e.code != ErrorCode::Cancelled {
                return Err(e);
            }
        }
        let skipped = if out.files == 0 && out.rows == 0 && out.in_use > 0 {
            Some(Skipped::InUse)
        } else if out.touched == 0 && out.unsupported > 0 {
            Some(Skipped::Unsupported)
        } else {
            None
        };
        if skipped.is_none() {
            ran_ids.push(rule.id.clone());
        }
        tf += out.files;
        tb += out.bytes;
        tr += out.rows;
        results.push(RuleClean {
            rule_id: rule.id.clone(),
            removed_files: out.files,
            removed_bytes: out.bytes,
            removed_rows: out.rows,
            failed: out.errors,
            skipped,
            actions: out.actions,
            running_apps: Vec::new(),
            closed_apps,
        });
        if was_cancelled {
            cancelled = true;
            break;
        }
    }
    job.progress(
        ProgressEvent::new("clean")
            .fraction(1.0)
            .counts(total, total),
    );

    let mut history_id = None;
    if !ran_ids.is_empty() {
        let id = random_id();
        // A history write failure must not turn a completed clean into an error.
        let entry = HistoryEntry {
            id: id.clone(),
            at: now_rfc3339(),
            total_bytes: tb,
            total_files: tf,
            total_rows: tr,
            rule_ids: ran_ids,
            source,
        };
        if history::append(ctx, entry).is_ok() {
            history_id = Some(id);
        }
    }

    Ok(CleanReport {
        results,
        total_files: tf,
        total_bytes: tb,
        total_rows: tr,
        duration_ms: start.elapsed().as_millis() as u64,
        cancelled,
        history_id,
    })
}

// ---------------------------------------------------------------- handlers

fn parse_params<T: DeserializeOwned + Default>(v: Value) -> Result<T> {
    if v.is_null() {
        Ok(T::default())
    } else {
        Ok(serde_json::from_value(v)?)
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AnalyzeParams {
    rule_ids: Option<Vec<String>>,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CleanParams {
    rule_ids: Option<Vec<String>>,
    close_apps: Option<CloseBrowsers>,
}

#[derive(serde::Deserialize, Default)]
struct HistoryParams {
    limit: Option<usize>,
}

fn list_rules_handler(ctx: &Ctx, _p: Value, _job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(list_rules(ctx))?)
}

fn analyze_handler(ctx: &Ctx, p: Value, job: &Job) -> Result<Value> {
    let p: AnalyzeParams = parse_params(p)?;
    Ok(serde_json::to_value(analyze(ctx, p.rule_ids, job)?)?)
}

fn clean_handler(ctx: &Ctx, p: Value, job: &Job) -> Result<Value> {
    let p: CleanParams = parse_params(p)?;
    let s = settings::load(ctx);
    let opts = CleanOptions::new(p.close_apps.unwrap_or(s.close_browsers));
    Ok(serde_json::to_value(run_clean(
        ctx,
        p.rule_ids,
        &opts,
        Source::Manual,
        job,
    )?)?)
}

fn history_handler(ctx: &Ctx, p: Value, _job: &Job) -> Result<Value> {
    let p: HistoryParams = parse_params(p)?;
    Ok(serde_json::to_value(history::list(ctx, p.limit))?)
}

#[cfg(test)]
mod tests;
