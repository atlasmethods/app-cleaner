//! Schedule data, validation and the "when was it last due" calendar arithmetic.

use serde::{Deserialize, Serialize};

use crate::clock::{civil_from_days, iso_weekday, LocalTime};
use crate::error::{ApiError, Result};

pub const MAX_NAME: usize = 60;
pub const MAX_SCHEDULES: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Frequency {
    Hourly,
    Daily,
    Weekly,
    Monthly,
    OnLogin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    #[default]
    Clean,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Action {
    pub kind: ActionKind,
    /// `None` = the rules enabled in Settings at run time.
    #[serde(default)]
    pub rules: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastResult {
    pub ok: bool,
    pub total_bytes: u64,
    pub total_files: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn default_time() -> String {
    "03:00".to_string()
}
fn default_dom() -> u8 {
    1
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub frequency: Frequency,
    /// `HH:MM`, 24 hours, local time. For `hourly` only the minutes matter.
    #[serde(default = "default_time")]
    pub time: String,
    /// ISO weekdays, 1 = Monday .. 7 = Sunday (weekly).
    #[serde(default)]
    pub weekdays: Vec<u8>,
    /// 1..=28 (monthly).
    #[serde(default = "default_dom")]
    pub day_of_month: u8,
    #[serde(default)]
    pub action: Action,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<LastResult>,
}

/// Body of `scheduler.add`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewSchedule {
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub frequency: Frequency,
    #[serde(default = "default_time")]
    pub time: String,
    #[serde(default)]
    pub weekdays: Vec<u8>,
    #[serde(default = "default_dom")]
    pub day_of_month: u8,
    #[serde(default)]
    pub action: Action,
}

/// Body of `scheduler.update`: every field but `id` is optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSchedule {
    pub id: String,
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub frequency: Option<Frequency>,
    pub time: Option<String>,
    pub weekdays: Option<Vec<u8>>,
    pub day_of_month: Option<u8>,
    pub action: Option<Action>,
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::invalid_params(msg)
}

/// Ids are 16 lower-case hex digits: safe in file names, unit names and command lines.
pub fn valid_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `(hour, minute)` of an `HH:MM` string.
pub fn parse_time(t: &str) -> Option<(u8, u8)> {
    let b = t.as_bytes();
    if b.len() != 5 || b[2] != b':' {
        return None;
    }
    let digit = |i: usize| b[i].is_ascii_digit().then(|| b[i] - b'0');
    let h = digit(0)? * 10 + digit(1)?;
    let m = digit(3)? * 10 + digit(4)?;
    (h < 24 && m < 60).then_some((h, m))
}

impl Schedule {
    /// Check and normalise in place. `others` are the other schedules (names must be unique
    /// among them); `known_rules` are the rule ids that exist on this system.
    pub fn validate(&mut self, others: &[Schedule], known_rules: &[String]) -> Result<()> {
        if !valid_id(&self.id) {
            return Err(bad("invalid schedule id"));
        }
        let name = self.name.trim().to_string();
        if name.is_empty() {
            return Err(bad("the schedule needs a name"));
        }
        if name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
            return Err(bad(format!(
                "the name must be at most {MAX_NAME} characters, without control characters"
            )));
        }
        if others
            .iter()
            .any(|o| o.id != self.id && o.name.to_lowercase() == name.to_lowercase())
        {
            return Err(bad(format!("there is already a schedule named `{name}`")));
        }
        self.name = name;

        let (h, m) = parse_time(&self.time)
            .ok_or_else(|| bad("time must look like 03:00 (24 hours, HH:MM)"))?;
        self.time = format!("{h:02}:{m:02}");

        if self.weekdays.iter().any(|d| !(1..=7).contains(d)) {
            return Err(bad(
                "weekdays must be numbers from 1 (Monday) to 7 (Sunday)",
            ));
        }
        self.weekdays.sort_unstable();
        self.weekdays.dedup();
        if self.frequency == Frequency::Weekly && self.weekdays.is_empty() {
            return Err(bad("choose at least one weekday for a weekly schedule"));
        }
        if !(1..=28).contains(&self.day_of_month) {
            return Err(bad("dayOfMonth must be between 1 and 28"));
        }

        if let Some(rules) = &mut self.action.rules {
            let mut out: Vec<String> = Vec::new();
            for r in rules.iter() {
                if !out.contains(r) {
                    out.push(r.clone());
                }
            }
            if out.is_empty() {
                return Err(bad(
                    "select at least one rule, or leave the rules unset to use the ones enabled in Settings",
                ));
            }
            if let Some(unknown) = out.iter().find(|r| !known_rules.contains(r)) {
                return Err(bad(format!("unknown rule `{unknown}` on this system")));
            }
            *rules = out;
        }
        Ok(())
    }

    /// `(hour, minute)`; validated schedules always parse.
    pub fn hm(&self) -> (u8, u8) {
        parse_time(&self.time).unwrap_or((3, 0))
    }
}

/// The most recent time `s` was due at or before `now`, in local minutes
/// (see [`LocalTime::local_minutes`]). `None` for `on_login`.
pub fn previous_due_minutes(s: &Schedule, now: &LocalTime) -> Option<i64> {
    let (h, m) = s.hm();
    let now_min = now.local_minutes();
    let day_start = now_min.div_euclid(1440) * 1440;
    match s.frequency {
        Frequency::OnLogin => None,
        Frequency::Hourly => {
            let hour_start = now_min.div_euclid(60) * 60;
            let cand = hour_start + i64::from(m);
            Some(if cand <= now_min { cand } else { cand - 60 })
        }
        Frequency::Daily => {
            let cand = day_start + i64::from(h) * 60 + i64::from(m);
            Some(if cand <= now_min { cand } else { cand - 1440 })
        }
        Frequency::Weekly | Frequency::Monthly => {
            let today = day_start / 1440;
            // At most a month and a bit back.
            for back in 0..=62i64 {
                let day = today - back;
                let hit = if s.frequency == Frequency::Weekly {
                    s.weekdays.contains(&iso_weekday(day))
                } else {
                    civil_from_days(day).2 == s.day_of_month
                };
                if !hit {
                    continue;
                }
                let cand = day * 1440 + i64::from(h) * 60 + i64::from(m);
                if cand <= now_min {
                    return Some(cand);
                }
            }
            None
        }
    }
}
