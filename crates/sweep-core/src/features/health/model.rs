//! Wire types of the health API (camelCase JSON).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::features::startup::Impact;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Good,
    Warning,
    Problem,
    /// The category could not be computed (the reason is in `summary`).
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CategoryId {
    Privacy,
    Space,
    Speed,
    Security,
}

impl CategoryId {
    pub const ALL: [CategoryId; 4] = [
        CategoryId::Privacy,
        CategoryId::Space,
        CategoryId::Speed,
        CategoryId::Security,
    ];

    pub fn title(self) -> &'static str {
        match self {
            CategoryId::Privacy => "Privacy",
            CategoryId::Space => "Space",
            CategoryId::Speed => "Speed",
            CategoryId::Security => "Security",
        }
    }
}

/// Metric keys of [`CategoryReport::metrics`] (the numbers the score is computed from).
pub mod metric {
    pub const BYTES: &str = "bytes";
    pub const FILES: &str = "files";
    pub const ROWS: &str = "rows";
    pub const TRACKERS: &str = "trackers";
    pub const HISTORY_ROWS: &str = "historyRows";
    pub const HIGH_IMPACT: &str = "highImpact";
    pub const MEDIUM_IMPACT: &str = "mediumImpact";
    pub const STARTUP_ITEMS: &str = "startupItems";
    pub const BACKGROUND_APPS: &str = "backgroundApps";
    pub const BACKGROUND_MEMORY: &str = "backgroundMemoryBytes";
    pub const UPDATES: &str = "updates";
    pub const SECURITY_UPDATES: &str = "securityUpdates";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupFinding {
    pub id: String,
    pub name: String,
    pub impact: Impact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppFinding {
    pub app_id: String,
    pub name: String,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFinding {
    pub id: String,
    pub name: String,
    pub current_version: String,
    pub new_version: String,
    pub security: bool,
}

/// One thing a category found. `kind` is the discriminator on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Finding {
    /// Cookies of sites that are not on the keep list.
    Trackers {
        count: u64,
        browsers: Vec<String>,
    },
    /// Browsing / download history rows.
    History {
        count: u64,
        browsers: Vec<String>,
    },
    /// Junk of one application or system area.
    Junk {
        group: String,
        bytes: u64,
        files: u64,
        rows: u64,
    },
    Startup {
        items: Vec<StartupFinding>,
    },
    BackgroundApps {
        apps: Vec<AppFinding>,
    },
    Updates {
        count: u64,
        security: u64,
        items: Vec<UpdateFinding>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryReport {
    pub id: CategoryId,
    pub title: String,
    pub status: Status,
    pub summary: String,
    pub findings: Vec<Finding>,
    /// There is something `health.fix` can do for this category.
    pub fixable: bool,
    /// The raw numbers behind the summary and the score (see [`metric`]).
    #[serde(default)]
    pub metrics: BTreeMap<String, u64>,
}

impl CategoryReport {
    pub fn unavailable(id: CategoryId, message: impl Into<String>) -> Self {
        Self {
            id,
            title: id.title().to_string(),
            status: Status::Unavailable,
            summary: message.into(),
            findings: Vec::new(),
            fixable: false,
            metrics: BTreeMap::new(),
        }
    }

    pub fn metric(&self, key: &str) -> u64 {
        self.metrics.get(key).copied().unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    /// 0..=100, or `null` when no category could be computed.
    pub score: Option<u8>,
    /// RFC 3339, UTC.
    pub scanned_at: String,
    pub categories: Vec<CategoryReport>,
}

// ---------------------------------------------------------------- fix

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Part {
    Privacy,
    Space,
    Startup,
    Sleep,
    Updates,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartStatus {
    /// Everything asked for was done.
    Done,
    /// Some of it was done, some was not (see `items` / `skipped`).
    Partial,
    /// Nothing was done (see `message`).
    Failed,
    /// Not started: the fix was cancelled first.
    NotRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemResult {
    pub id: String,
    pub name: String,
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartResult {
    pub part: Part,
    pub status: PartStatus,
    pub message: String,
    pub removed_bytes: u64,
    pub removed_files: u64,
    pub removed_rows: u64,
    /// Privacy / space: apps that were running, so their data was not cleaned
    /// (`skipped == "app_running"` in the cleaner's terms). Retry with `closeApps: "always"`.
    pub blocked_apps: Vec<String>,
    /// Startup / sleep / updates: one row per requested id.
    pub items: Vec<ItemResult>,
}

impl PartResult {
    pub fn new(part: Part, status: PartStatus, message: impl Into<String>) -> Self {
        Self {
            part,
            status,
            message: message.into(),
            removed_bytes: 0,
            removed_files: 0,
            removed_rows: 0,
            blocked_apps: Vec::new(),
            items: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixReport {
    pub parts: Vec<PartResult>,
    /// The fix was cancelled between (or during) parts; `report` is then `null`.
    pub cancelled: bool,
    /// A fresh analysis after the fix (also stored as the last result).
    pub report: Option<HealthReport>,
}
