//! The health score: a deterministic, documented formula.
//!
//! The score starts at 100 and loses points for what each category found. Every category has
//! a **cap** (the most it can take away) and the caps add up to 100:
//!
//! | category | cap | penalty before the cap |
//! |----------|-----|------------------------|
//! | space    | 25  | 10 per GiB of junk |
//! | privacy  | 20  | 0.2 per tracking cookie + 0.02 per history / download row |
//! | speed    | 25  | 6 per high-impact startup item + 2 per medium-impact one + 1 per background app |
//! | security | 30  | 1 per available update + 4 more for each security update (so 5 in all) |
//!
//! A category that is *unavailable* is left out and the remaining penalties are scaled by
//! `100 / (sum of the remaining caps)`, so the score still spans 0..=100 (weights are
//! renormalized). The result is rounded to the nearest whole number. With no available
//! category there is no score (`None`).

use super::model::{metric, CategoryId, CategoryReport, Status};

pub const CAP_SPACE: f64 = 25.0;
pub const CAP_PRIVACY: f64 = 20.0;
pub const CAP_SPEED: f64 = 25.0;
pub const CAP_SECURITY: f64 = 30.0;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

pub const PER_GIB_JUNK: f64 = 10.0;
pub const PER_TRACKER: f64 = 0.2;
pub const PER_HISTORY_ROW: f64 = 0.02;
pub const PER_HIGH_IMPACT: f64 = 6.0;
pub const PER_MEDIUM_IMPACT: f64 = 2.0;
pub const PER_BACKGROUND_APP: f64 = 1.0;
pub const PER_UPDATE: f64 = 1.0;
pub const PER_SECURITY_UPDATE: f64 = 5.0;

/// What a category found, as far as the score is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpaceIn {
    pub junk_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PrivacyIn {
    pub trackers: u64,
    pub history_rows: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpeedIn {
    pub high_impact: u64,
    pub medium_impact: u64,
    pub background_apps: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SecurityIn {
    /// All available updates, security ones included.
    pub updates: u64,
    pub security: u64,
}

/// `None` = the category is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Inputs {
    pub space: Option<SpaceIn>,
    pub privacy: Option<PrivacyIn>,
    pub speed: Option<SpeedIn>,
    pub security: Option<SecurityIn>,
}

pub fn space_penalty(i: SpaceIn) -> f64 {
    (i.junk_bytes as f64 / GIB * PER_GIB_JUNK).min(CAP_SPACE)
}

pub fn privacy_penalty(i: PrivacyIn) -> f64 {
    (i.trackers as f64 * PER_TRACKER + i.history_rows as f64 * PER_HISTORY_ROW).min(CAP_PRIVACY)
}

pub fn speed_penalty(i: SpeedIn) -> f64 {
    (i.high_impact as f64 * PER_HIGH_IMPACT
        + i.medium_impact as f64 * PER_MEDIUM_IMPACT
        + i.background_apps as f64 * PER_BACKGROUND_APP)
        .min(CAP_SPEED)
}

pub fn security_penalty(i: SecurityIn) -> f64 {
    let regular = i.updates.saturating_sub(i.security);
    (regular as f64 * PER_UPDATE + i.security as f64 * PER_SECURITY_UPDATE).min(CAP_SECURITY)
}

/// The overall score, 0..=100, or `None` when no category is available.
pub fn overall(inputs: &Inputs) -> Option<u8> {
    let mut caps = 0.0;
    let mut penalty = 0.0;
    if let Some(i) = inputs.space {
        caps += CAP_SPACE;
        penalty += space_penalty(i);
    }
    if let Some(i) = inputs.privacy {
        caps += CAP_PRIVACY;
        penalty += privacy_penalty(i);
    }
    if let Some(i) = inputs.speed {
        caps += CAP_SPEED;
        penalty += speed_penalty(i);
    }
    if let Some(i) = inputs.security {
        caps += CAP_SECURITY;
        penalty += security_penalty(i);
    }
    if caps <= 0.0 {
        return None;
    }
    let score = 100.0 - penalty * (100.0 / caps);
    Some(score.round().clamp(0.0, 100.0) as u8)
}

/// Read the inputs back from the categories' metrics.
pub fn inputs_of(categories: &[CategoryReport]) -> Inputs {
    let mut inputs = Inputs::default();
    for c in categories {
        if c.status == Status::Unavailable {
            continue;
        }
        match c.id {
            CategoryId::Space => {
                inputs.space = Some(SpaceIn {
                    junk_bytes: c.metric(metric::BYTES),
                });
            }
            CategoryId::Privacy => {
                inputs.privacy = Some(PrivacyIn {
                    trackers: c.metric(metric::TRACKERS),
                    history_rows: c.metric(metric::HISTORY_ROWS),
                });
            }
            CategoryId::Speed => {
                inputs.speed = Some(SpeedIn {
                    high_impact: c.metric(metric::HIGH_IMPACT),
                    medium_impact: c.metric(metric::MEDIUM_IMPACT),
                    background_apps: c.metric(metric::BACKGROUND_APPS),
                });
            }
            CategoryId::Security => {
                inputs.security = Some(SecurityIn {
                    updates: c.metric(metric::UPDATES),
                    security: c.metric(metric::SECURITY_UPDATES),
                });
            }
        }
    }
    inputs
}

/// Score of a set of category reports.
pub fn score_of(categories: &[CategoryReport]) -> Option<u8> {
    overall(&inputs_of(categories))
}
