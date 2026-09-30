//! The four category scans. Each one is computed on its own: an error in one makes that
//! category `unavailable` (with the message) and never fails the whole analysis.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::mpsc;

use crate::ctx::Ctx;
use crate::error::{ErrorCode, Result};
use crate::features::cleaner::rules::{Category, Rule};
use crate::features::cleaner::{self, AnalyzeReport};
use crate::features::optimizer::{self, apps::{entry_app_key, is_security_software}};
use crate::features::settings::{self, Settings};
use crate::features::software_updater;
use crate::features::startup::{self, Entry, Impact, Kind};
use crate::fsutil::now_rfc3339;
use crate::job::{Job, ProgressEvent};

use super::model::{
    metric, AppFinding, CategoryId, CategoryReport, Finding, HealthReport, StartupFinding, Status,
    UpdateFinding,
};
use super::score;

/// How many junk groups / apps / updates a finding list carries.
const TOP_GROUPS: usize = 5;
const TOP_APPS: usize = 20;
const MAX_UPDATES: usize = 200;

/// Junk of 1 GiB or more is a "problem"; tracking cookies from this many on likewise.
const PROBLEM_JUNK_BYTES: u64 = 1024 * 1024 * 1024;
const PROBLEM_TRACKERS: u64 = 100;

// ---------------------------------------------------------------- progress plumbing

/// Run `f` with a job that shares `job`'s cancellation and forwards its progress scaled
/// into `base..base + span` of `job`'s own bar, prefixed with `label`.
pub fn scoped<T>(job: &Job, base: f64, span: f64, label: &str, f: impl FnOnce(&Job) -> T) -> T {
    let (tx, rx) = mpsc::channel::<ProgressEvent>();
    let inner = Job::new(job.token().clone(), move |e| {
        let _ = tx.send(e);
    });
    std::thread::scope(|s| {
        s.spawn(|| {
            for ev in rx {
                let frac = base + span * ev.fraction.unwrap_or(0.0);
                let msg = match &ev.message {
                    Some(m) => format!("{label}: {m}"),
                    None => label.to_string(),
                };
                job.progress(ProgressEvent::new("health").fraction(frac).message(msg));
            }
        });
        let out = f(&inner);
        // Dropping the job closes the channel, which ends the forwarding thread.
        drop(inner);
        out
    })
}

// ---------------------------------------------------------------- wording

pub fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

// ---------------------------------------------------------------- rule selection

/// Browser rules that hold tracking data: cookies, history and download history.
fn is_privacy_rule(r: &Rule) -> bool {
    r.category == Category::Browser
        && (r.id.ends_with(".cookies") || r.id.ends_with(".history") || r.id.ends_with(".downloads"))
}

/// The enabled rules (the Clean tab's selection) split into (privacy, space).
pub fn enabled_rule_ids(ctx: &Ctx, s: &Settings) -> Result<(Vec<String>, Vec<String>)> {
    let enabled = cleaner::select_rules(ctx, s, None)?;
    let (privacy, space): (Vec<Rule>, Vec<Rule>) = enabled.into_iter().partition(is_privacy_rule);
    Ok((
        privacy.into_iter().map(|r| r.id).collect(),
        space.into_iter().map(|r| r.id).collect(),
    ))
}

fn analyze_rules(ctx: &Ctx, ids: Vec<String>, job: &Job) -> Result<Option<AnalyzeReport>> {
    if ids.is_empty() {
        return Ok(None);
    }
    Ok(Some(cleaner::analyze(ctx, Some(ids), job)?))
}

// ---------------------------------------------------------------- privacy

pub fn privacy(ctx: &Ctx, job: &Job) -> Result<CategoryReport> {
    let s = settings::load(ctx);
    let (ids, _) = enabled_rule_ids(ctx, &s)?;
    let report = analyze_rules(ctx, ids, job)?;
    let mut trackers = 0u64;
    let mut history = 0u64;
    let mut tracker_browsers = BTreeSet::new();
    let mut history_browsers = BTreeSet::new();
    if let Some(r) = &report {
        for it in &r.items {
            if it.rows == 0 {
                continue;
            }
            if it.rule_id.ends_with(".cookies") {
                trackers += it.rows;
                tracker_browsers.insert(it.group.clone());
            } else {
                history += it.rows;
                history_browsers.insert(it.group.clone());
            }
        }
    }
    let mut findings = Vec::new();
    if trackers > 0 {
        findings.push(Finding::Trackers {
            count: trackers,
            browsers: tracker_browsers.iter().cloned().collect(),
        });
    }
    if history > 0 {
        findings.push(Finding::History {
            count: history,
            browsers: history_browsers.iter().cloned().collect(),
        });
    }
    let fixable = trackers > 0 || history > 0;
    let status = if !fixable {
        Status::Good
    } else if trackers >= PROBLEM_TRACKERS {
        Status::Problem
    } else {
        Status::Warning
    };
    let summary = if !fixable {
        "No tracking cookies or browsing traces found.".to_string()
    } else {
        let mut parts = Vec::new();
        if trackers > 0 {
            parts.push(format!(
                "{} in {}",
                plural(trackers, "tracking cookie", "tracking cookies"),
                plural(tracker_browsers.len() as u64, "browser", "browsers")
            ));
        }
        if history > 0 {
            parts.push(plural(history, "history entry", "history entries"));
        }
        format!("{}.", parts.join(", "))
    };
    Ok(CategoryReport {
        id: CategoryId::Privacy,
        title: CategoryId::Privacy.title().into(),
        status,
        summary,
        findings,
        fixable,
        metrics: BTreeMap::from([
            (metric::TRACKERS.to_string(), trackers),
            (metric::HISTORY_ROWS.to_string(), history),
        ]),
    })
}

// ---------------------------------------------------------------- space

pub fn space(ctx: &Ctx, job: &Job) -> Result<CategoryReport> {
    let s = settings::load(ctx);
    let (_, ids) = enabled_rule_ids(ctx, &s)?;
    let report = analyze_rules(ctx, ids, job)?;
    let (mut bytes, mut files, mut rows) = (0u64, 0u64, 0u64);
    let mut groups: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    if let Some(r) = &report {
        for it in &r.items {
            bytes += it.bytes;
            files += it.files;
            rows += it.rows;
            if it.bytes > 0 || it.files > 0 || it.rows > 0 {
                let g = groups.entry(it.group.clone()).or_default();
                g.0 += it.bytes;
                g.1 += it.files;
                g.2 += it.rows;
            }
        }
    }
    let mut ranked: Vec<(String, (u64, u64, u64))> = groups.into_iter().collect();
    ranked.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then_with(|| a.0.cmp(&b.0)));
    let findings: Vec<Finding> = ranked
        .into_iter()
        .take(TOP_GROUPS)
        .map(|(group, (bytes, files, rows))| Finding::Junk {
            group,
            bytes,
            files,
            rows,
        })
        .collect();
    let fixable = bytes > 0 || files > 0 || rows > 0;
    let status = if !fixable {
        Status::Good
    } else if bytes >= PROBLEM_JUNK_BYTES {
        Status::Problem
    } else {
        Status::Warning
    };
    let summary = if !fixable {
        "No junk files found.".to_string()
    } else {
        format!(
            "{} of junk in {}.",
            fmt_bytes(bytes),
            plural(files, "file", "files")
        )
    };
    Ok(CategoryReport {
        id: CategoryId::Space,
        title: CategoryId::Space.title().into(),
        status,
        summary,
        findings,
        fixable,
        metrics: BTreeMap::from([
            (metric::BYTES.to_string(), bytes),
            (metric::FILES.to_string(), files),
            (metric::ROWS.to_string(), rows),
        ]),
    })
}

// ---------------------------------------------------------------- speed

/// Startup items the Health Check may offer to switch off (and `fix` accepts): user-level,
/// enabled, not critical, switchable, no administrator rights needed, and of a kind that is
/// a plain "launch at login" item (services, scheduled tasks and context-menu handlers are
/// left to the Startup page).
pub fn startup_candidate(e: &Entry) -> bool {
    e.item.enabled
        && !e.item.critical
        && e.item.can_disable
        && !startup::needs_admin(&e.target)
        && matches!(e.item.kind, Kind::Autostart | Kind::LaunchAgent)
        // Security software is never switched off (the optimizer protects it the same way).
        && !is_security_software(&entry_app_key(e))
        && !is_security_software(&e.item.name)
}

/// Background apps: running, not asleep, not protected and started automatically by an
/// enabled startup item (so putting them to sleep has something to switch off).
pub fn background_apps(a: &optimizer::Analysis) -> Vec<(String, String, u64)> {
    if a.state_error.is_some() {
        // The sleep record is unreadable: sleeping anything would fail.
        return Vec::new();
    }
    let enabled: HashSet<&str> = a
        .entries
        .iter()
        .filter(|e| e.item.enabled)
        .map(|e| e.item.id.as_str())
        .collect();
    let mut out: Vec<(String, String, u64)> = a
        .apps
        .iter()
        .filter(|app| {
            !app.protected
                && !app.procs.is_empty()
                && !a.state.sleeping.contains_key(&app.id)
                && app
                    .startup_ids
                    .iter()
                    .chain(app.service_ids.iter())
                    .any(|id| enabled.contains(id.as_str()))
        })
        .map(|app| {
            (
                app.id.clone(),
                app.name.clone(),
                app.procs.iter().map(|p| p.memory_bytes).sum(),
            )
        })
        .collect();
    out.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    out
}

pub fn speed(ctx: &Ctx, job: &Job) -> Result<CategoryReport> {
    let a = optimizer::analyze(ctx, job)?;
    let mut items: Vec<StartupFinding> = a
        .entries
        .iter()
        .filter(|e| startup_candidate(e) && matches!(e.item.impact, Impact::High | Impact::Medium))
        .map(|e| StartupFinding {
            id: e.item.id.clone(),
            name: e.item.name.clone(),
            impact: e.item.impact,
        })
        .collect();
    items.sort_by(|x, y| {
        (x.impact != Impact::High, x.name.to_lowercase())
            .cmp(&(y.impact != Impact::High, y.name.to_lowercase()))
    });
    let high = items.iter().filter(|i| i.impact == Impact::High).count() as u64;
    let medium = items.len() as u64 - high;
    let apps = background_apps(&a);
    let mem: u64 = apps.iter().map(|a| a.2).sum();
    let app_count = apps.len() as u64;

    let mut findings = Vec::new();
    let item_count = items.len() as u64;
    if !items.is_empty() {
        findings.push(Finding::Startup { items });
    }
    if !apps.is_empty() {
        findings.push(Finding::BackgroundApps {
            apps: apps
                .into_iter()
                .take(TOP_APPS)
                .map(|(app_id, name, memory_bytes)| AppFinding {
                    app_id,
                    name,
                    memory_bytes,
                })
                .collect(),
        });
    }
    let fixable = item_count > 0 || app_count > 0;
    let status = if !fixable {
        Status::Good
    } else if high > 0 {
        Status::Problem
    } else {
        Status::Warning
    };
    let summary = if !fixable {
        "Nothing is slowing down startup or running in the background.".to_string()
    } else {
        let mut parts = Vec::new();
        if item_count > 0 {
            parts.push(format!(
                "{} slow down startup",
                plural(item_count, "startup item", "startup items")
            ));
        }
        if app_count > 0 {
            parts.push(format!(
                "{} in the background ({})",
                plural(app_count, "app runs", "apps run"),
                fmt_bytes(mem)
            ));
        }
        format!("{}.", parts.join("; "))
    };
    Ok(CategoryReport {
        id: CategoryId::Speed,
        title: CategoryId::Speed.title().into(),
        status,
        summary,
        findings,
        fixable,
        metrics: BTreeMap::from([
            (metric::HIGH_IMPACT.to_string(), high),
            (metric::MEDIUM_IMPACT.to_string(), medium),
            (metric::STARTUP_ITEMS.to_string(), item_count),
            (metric::BACKGROUND_APPS.to_string(), app_count),
            (metric::BACKGROUND_MEMORY.to_string(), mem),
        ]),
    })
}

// ---------------------------------------------------------------- security

pub fn security(ctx: &Ctx, job: &Job) -> Result<CategoryReport> {
    // No index refresh: this must be fast (and must never ask for administrator rights).
    let items = software_updater::collect(ctx, job, false)?;
    let ignored: HashSet<String> = settings::load(ctx).ignored_updates.into_iter().collect();
    let items: Vec<_> = items
        .into_iter()
        .filter(|i| !ignored.contains(&i.id()))
        .collect();
    let count = items.len() as u64;
    let sec = items.iter().filter(|i| i.security == Some(true)).count() as u64;
    let mut listed: Vec<UpdateFinding> = items
        .iter()
        .map(|i| UpdateFinding {
            id: i.id(),
            name: i.name.clone(),
            current_version: i.current.clone(),
            new_version: i.new.clone(),
            security: i.security == Some(true),
        })
        .collect();
    // Security updates first.
    listed.sort_by_key(|u| !u.security);
    listed.truncate(MAX_UPDATES);
    let findings = if count > 0 {
        vec![Finding::Updates {
            count,
            security: sec,
            items: listed,
        }]
    } else {
        Vec::new()
    };
    let status = if sec > 0 {
        Status::Problem
    } else if count > 0 {
        Status::Warning
    } else {
        Status::Good
    };
    let summary = if count == 0 {
        "All software is up to date.".to_string()
    } else if sec > 0 {
        format!(
            "{} available, {} security.",
            plural(count, "update", "updates"),
            sec
        )
    } else {
        format!("{} available.", plural(count, "update", "updates"))
    };
    Ok(CategoryReport {
        id: CategoryId::Security,
        title: CategoryId::Security.title().into(),
        status,
        summary,
        findings,
        fixable: count > 0,
        metrics: BTreeMap::from([
            (metric::UPDATES.to_string(), count),
            (metric::SECURITY_UPDATES.to_string(), sec),
        ]),
    })
}

// ---------------------------------------------------------------- everything

/// Compute one category. Any failure except cancellation becomes `unavailable`.
pub(super) fn category(id: CategoryId, ctx: &Ctx, job: &Job) -> Result<CategoryReport> {
    let r = match id {
        CategoryId::Privacy => privacy(ctx, job),
        CategoryId::Space => space(ctx, job),
        CategoryId::Speed => speed(ctx, job),
        CategoryId::Security => security(ctx, job),
    };
    match r {
        Ok(c) => Ok(c),
        Err(e) if e.code == ErrorCode::Cancelled => Err(e),
        Err(e) => Ok(CategoryReport::unavailable(id, e.message)),
    }
}

/// Scan all four categories. Progress is reported per category.
pub fn analyze(ctx: &Ctx, job: &Job) -> Result<HealthReport> {
    let n = CategoryId::ALL.len() as f64;
    let mut categories = Vec::with_capacity(CategoryId::ALL.len());
    for (i, id) in CategoryId::ALL.into_iter().enumerate() {
        job.check_cancelled()?;
        let base = i as f64 / n;
        let label = format!("Checking {}", id.title().to_lowercase());
        job.progress(
            ProgressEvent::new("health")
                .fraction(base)
                .counts(i as u64, n as u64)
                .message(label.clone()),
        );
        categories.push(scoped(job, base, 1.0 / n, &label, |inner| {
            category(id, ctx, inner)
        })?);
    }
    job.progress(
        ProgressEvent::new("health")
            .fraction(1.0)
            .counts(n as u64, n as u64),
    );
    Ok(HealthReport {
        score: score::score_of(&categories),
        scanned_at: now_rfc3339(),
        categories,
    })
}
