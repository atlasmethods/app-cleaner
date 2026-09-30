//! Wire types of the cleaner API (camelCase JSON).

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::features::cleaner::rules::Category;
use crate::features::settings::{CloseBrowsers, Settings};

pub const MAX_SAMPLES: usize = 50;
pub const MAX_ANALYZE_ERRORS: usize = 20;
pub const MAX_CLEAN_FAILURES: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathErr {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    pub enabled: bool,
    pub default_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInfo {
    pub group: String,
    pub rules: Vec<RuleInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryInfo {
    pub category: Category,
    pub label: String,
    pub groups: Vec<GroupInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulesListing {
    pub categories: Vec<CategoryInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeItem {
    pub rule_id: String,
    pub name: String,
    pub group: String,
    pub category: Category,
    pub files: u64,
    pub bytes: u64,
    pub rows: u64,
    pub sample_paths: Vec<String>,
    pub app_running: bool,
    pub errors: Vec<PathErr>,
    pub actions: Vec<String>,
    /// Temp folders / files left alone because a program still uses them (not an error).
    #[serde(default)]
    pub in_use_skipped: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeReport {
    pub items: Vec<AnalyzeItem>,
    pub total_files: u64,
    pub total_bytes: u64,
    pub total_rows: u64,
    pub duration_ms: u64,
}

/// Why a rule was not cleaned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Skipped {
    AppRunning,
    InUse,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuleClean {
    pub rule_id: String,
    pub removed_files: u64,
    pub removed_bytes: u64,
    pub removed_rows: u64,
    pub failed: Vec<PathErr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<Skipped>,
    /// Programs/actions that ran (clipboard cleared, DNS flushed, ...).
    pub actions: Vec<String>,
    /// For `skipped == app_running`: names of the apps that must be closed.
    pub running_apps: Vec<String>,
    /// Apps that were asked to close (and did) before cleaning.
    pub closed_apps: Vec<String>,
    /// Temp folders / files left alone because a program still uses them (not an error).
    #[serde(default)]
    pub in_use_skipped: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CleanReport {
    pub results: Vec<RuleClean>,
    pub total_files: u64,
    pub total_bytes: u64,
    pub total_rows: u64,
    pub duration_ms: u64,
    /// The run was cancelled part-way; results cover what completed.
    pub cancelled: bool,
    /// Id of the history entry written for this run, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Manual,
    Auto,
    Smart,
    Scheduled,
    /// The Health Check's one-click fix.
    Health,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    /// RFC 3339, UTC.
    pub at: String,
    pub total_bytes: u64,
    pub total_files: u64,
    #[serde(default)]
    pub total_rows: u64,
    pub rule_ids: Vec<String>,
    pub source: Source,
}

/// How to treat rules whose application is running.
#[derive(Debug, Clone)]
pub struct CleanOptions {
    pub close_apps: CloseBrowsers,
    /// How long to wait for a closed app to exit before skipping its rules.
    pub close_timeout: Duration,
}

impl CleanOptions {
    pub fn new(close_apps: CloseBrowsers) -> Self {
        Self {
            close_apps,
            close_timeout: Duration::from_secs(10),
        }
    }
    /// The saved policy; with `unattended`, "ask" cannot be answered so it means "skip".
    pub fn from_settings(s: &Settings, unattended: bool) -> Self {
        let mut c = s.close_browsers;
        if unattended && c == CloseBrowsers::Ask {
            c = CloseBrowsers::Skip;
        }
        Self::new(c)
    }
}
