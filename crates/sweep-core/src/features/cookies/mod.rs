//! Cookie manager: list browser cookies and maintain a keep-list.
//!
//! Methods:
//! - `cookies.list`: `[{domain, count, browsers, kept}]` across all browser profiles.
//! - `cookies.get_keep_list` / `cookies.set_keep_list { domains }`: `{ domains }`.
//! - `cookies.intelligent_scan`: adds commonly used sites found in the cookie
//!   databases to the keep list; returns `{ domains, added }`.
//! - `cookies.delete { domains, closeApps? }`: deletes those domains' cookies (and their
//!   subdomains') in every browser. An explicit delete overrides the keep list.
//!
//! Cookie database locations come from the `cookies` targets of the cleaner rules, so
//! there is a single source of truth.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

use crate::api::Registry;
use crate::ctx::Ctx;
use crate::error::{ApiError, Result};
use crate::features::cleaner::apps;
use crate::features::cleaner::rules::{self, Family, Rule, Target};
use crate::features::cleaner::sqlite::{
    self, host_matches_any, host_matches_domain, normalize_host, CookieSelect, DbError,
};
use crate::features::cleaner::template;
use crate::features::settings::{self, CloseBrowsers};
use crate::job::Job;
use crate::safety::{key_of_path, ExcludeSet};

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "cookies.list",
    "cookies.get_keep_list",
    "cookies.set_keep_list",
    "cookies.intelligent_scan",
    "cookies.delete",
];

pub fn register(r: &mut Registry) {
    r.add("cookies.list", list);
    r.add("cookies.get_keep_list", get_keep_list);
    r.add("cookies.set_keep_list", set_keep_list);
    r.add("cookies.intelligent_scan", intelligent_scan);
    r.add("cookies.delete", delete);
}

/// Sites people commonly stay signed in to. `intelligent_scan` keeps the ones found.
pub const KNOWN_SITES: &[&str] = &[
    "google.com",
    "youtube.com",
    "github.com",
    "gitlab.com",
    "microsoft.com",
    "live.com",
    "outlook.com",
    "office.com",
    "microsoftonline.com",
    "apple.com",
    "icloud.com",
    "amazon.com",
    "amazon.co.uk",
    "amazon.de",
    "facebook.com",
    "instagram.com",
    "whatsapp.com",
    "x.com",
    "twitter.com",
    "linkedin.com",
    "reddit.com",
    "netflix.com",
    "spotify.com",
    "dropbox.com",
    "slack.com",
    "discord.com",
    "zoom.us",
    "paypal.com",
    "ebay.com",
    "yahoo.com",
    "proton.me",
    "protonmail.com",
    "notion.so",
    "atlassian.net",
    "atlassian.com",
    "stackoverflow.com",
    "stackexchange.com",
    "wikipedia.org",
    "twitch.tv",
    "tiktok.com",
    "pinterest.com",
    "tumblr.com",
    "medium.com",
    "dropboxusercontent.com",
    "box.com",
    "salesforce.com",
    "adobe.com",
    "shopify.com",
    "etsy.com",
    "walmart.com",
    "target.com",
    "bestbuy.com",
    "airbnb.com",
    "booking.com",
    "uber.com",
    "lyft.com",
    "cloudflare.com",
    "digitalocean.com",
    "heroku.com",
    "npmjs.com",
    "hulu.com",
    "disneyplus.com",
    "primevideo.com",
    "steampowered.com",
    "epicgames.com",
    "bitbucket.org",
    "figma.com",
    "canva.com",
    "trello.com",
    "asana.com",
    "monday.com",
    "chase.com",
    "bankofamerica.com",
    "wellsfargo.com",
];

/// Normalize a user-entered domain: lower-case, no scheme, path, port or leading dots.
/// Returns `None` if nothing usable is left.
pub fn normalize_domain(input: &str) -> Option<String> {
    let mut s = input.trim().to_lowercase();
    if let Some((_, rest)) = s.split_once("://") {
        s = rest.to_string();
    }
    if let Some(i) = s.find(['/', '?', '#']) {
        s.truncate(i);
    }
    if let Some((host, port)) = s.rsplit_once(':') {
        if !host.contains(':') && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
            s = host.to_string();
        }
    }
    let s = s.trim_start_matches("*.").trim_matches('.').to_string();
    if s.is_empty() || s.len() > 253 {
        return None;
    }
    let bad = |c: char| {
        c.is_whitespace()
            || c.is_control()
            || matches!(
                c,
                '/' | '\\' | '*' | '%' | ',' | ';' | '"' | '\'' | '<' | '>' | '@' | ':'
            )
    };
    if s.chars().any(bad) || s.contains("..") {
        return None;
    }
    Some(s)
}

// ---------------------------------------------------------------- database discovery

struct CookieDb<'a> {
    rule: &'a Rule,
    family: Family,
    path: PathBuf,
}

fn cookie_dbs<'a>(ctx: &Ctx, excludes: &ExcludeSet, rules: &'a [Rule]) -> Vec<CookieDb<'a>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for rule in rules {
        for t in rule.targets_for(ctx.env.os) {
            let Target::Cookies(c) = t else { continue };
            for f in template::resolve(&ctx.env, &c.db) {
                if !f.meta.file_type().is_file() || excludes.is_excluded(&f.path) {
                    continue;
                }
                if seen.insert(key_of_path(&f.path)) {
                    out.push(CookieDb {
                        rule,
                        family: c.browser_family,
                        path: f.path,
                    });
                }
            }
        }
    }
    out
}

fn cookie_rules(ctx: &Ctx) -> Vec<Rule> {
    rules::rules_for_os(ctx.env.os)
        .into_iter()
        .filter(|r| r.targets.iter().any(|t| matches!(t, Target::Cookies(_))))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------- listing

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CookieDomain {
    pub domain: String,
    pub count: u64,
    pub browsers: Vec<String>,
    pub kept: bool,
}

/// Aggregate cookies by normalized host across every browser profile found.
pub fn list_domains(ctx: &Ctx) -> Vec<CookieDomain> {
    let s = settings::load(ctx);
    // Listing is read-only, so an invalid exclusion list must not hide the cookies.
    let excludes = ExcludeSet::empty();
    let rules = cookie_rules(ctx);
    let mut agg: BTreeMap<String, (u64, BTreeSet<String>)> = BTreeMap::new();
    for db in cookie_dbs(ctx, &excludes, &rules) {
        let Ok(counts) = sqlite::cookie_counts(&db.path, db.family) else {
            continue;
        };
        for (host, n) in counts {
            let d = normalize_host(&host);
            if d.is_empty() {
                continue;
            }
            let e = agg.entry(d).or_default();
            e.0 += n;
            e.1.insert(db.rule.group.clone());
        }
    }
    agg.into_iter()
        .map(|(domain, (count, browsers))| CookieDomain {
            kept: host_matches_any(&domain, &s.cookie_keep),
            domain,
            count,
            browsers: browsers.into_iter().collect(),
        })
        .collect()
}

fn list(ctx: &Ctx, _p: Value, _job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(list_domains(ctx))?)
}

// ---------------------------------------------------------------- keep list

#[derive(Deserialize)]
struct DomainsParams {
    domains: Vec<String>,
}

fn keep_list_value(domains: &[String]) -> Value {
    serde_json::json!({ "domains": domains })
}

fn get_keep_list(ctx: &Ctx, _p: Value, _job: &Job) -> Result<Value> {
    Ok(keep_list_value(&settings::load(ctx).cookie_keep))
}

fn set_keep_list(ctx: &Ctx, p: Value, _job: &Job) -> Result<Value> {
    let p: DomainsParams = serde_json::from_value(p)?;
    let s = settings::update(ctx, |s| s.cookie_keep = p.domains)?;
    Ok(keep_list_value(&s.cookie_keep))
}

fn intelligent_scan(ctx: &Ctx, _p: Value, _job: &Job) -> Result<Value> {
    let present: Vec<String> = list_domains(ctx).into_iter().map(|d| d.domain).collect();
    let mut added = Vec::new();
    let s = settings::update(ctx, |s| {
        for site in KNOWN_SITES {
            let site = (*site).to_string();
            if s.cookie_keep.contains(&site) {
                continue;
            }
            if present.iter().any(|d| host_matches_domain(d, &site)) {
                s.cookie_keep.push(site.clone());
                added.push(site);
            }
        }
    })?;
    Ok(serde_json::json!({ "domains": s.cookie_keep, "added": added }))
}

// ---------------------------------------------------------------- delete

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteParams {
    domains: Vec<String>,
    close_apps: Option<CloseBrowsers>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDelete {
    pub browser: String,
    pub deleted: u64,
    /// `app_running` or `in_use` when nothing could be deleted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<&'static str>,
    pub errors: Vec<String>,
}

fn delete(ctx: &Ctx, p: Value, job: &Job) -> Result<Value> {
    let p: DeleteParams = serde_json::from_value(p)?;
    let mut domains = Vec::new();
    for d in &p.domains {
        let n = normalize_domain(d)
            .ok_or_else(|| ApiError::invalid_params(format!("invalid domain `{d}`")))?;
        if !domains.contains(&n) {
            domains.push(n);
        }
    }
    if domains.is_empty() {
        return Err(ApiError::invalid_params("`domains` is empty"));
    }
    let s = settings::load(ctx);
    let close = p.close_apps.unwrap_or(s.close_browsers);
    let excludes = ExcludeSet::from_settings(&ctx.env, &s)?;
    let rules = cookie_rules(ctx);
    let dbs = cookie_dbs(ctx, &excludes, &rules);
    let mut procs = ctx.procs.list();

    // Group by browser (rule) so a running browser is handled once.
    let mut by_rule: BTreeMap<String, Vec<&CookieDb>> = BTreeMap::new();
    for db in &dbs {
        by_rule.entry(db.rule.id.clone()).or_default().push(db);
    }
    let mut browsers: BTreeMap<String, BrowserDelete> = BTreeMap::new();
    let mut total = 0u64;
    for (_, group) in by_rule {
        job.check_cancelled()?;
        let rule = group[0].rule;
        let entry = browsers
            .entry(rule.group.clone())
            .or_insert_with(|| BrowserDelete {
                browser: rule.group.clone(),
                deleted: 0,
                skipped: None,
                errors: Vec::new(),
            });
        if apps::is_running(rule, ctx.env.os, &procs) {
            let mut ok = false;
            if close == CloseBrowsers::Always {
                let (fresh, gone) = apps::close_and_wait(
                    ctx,
                    rule,
                    &procs,
                    std::time::Duration::from_secs(10),
                    job,
                );
                procs = fresh;
                ok = gone;
            }
            if !ok {
                entry.skipped = Some("app_running");
                continue;
            }
        }
        let select = CookieSelect::Only(&domains);
        let mut in_use = 0;
        for db in group {
            match sqlite::delete_cookies(&db.path, db.family, &select) {
                Ok(o) => {
                    entry.deleted += o.rows;
                    total += o.rows;
                }
                Err(DbError::InUse) => {
                    in_use += 1;
                    entry.errors.push(format!("{}: in use", db.path.display()));
                }
                Err(DbError::Other(m)) => entry.errors.push(format!("{}: {m}", db.path.display())),
            }
        }
        if in_use > 0 && entry.deleted == 0 {
            entry.skipped = Some("in_use");
        }
    }
    Ok(serde_json::json!({
        "deleted": total,
        "browsers": browsers.into_values().collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests;
