//! Parsers for the output of the restore-point tools. Pure functions, unit-tested with captured
//! output. They never fail: text they do not understand yields no rows.

use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

/// A point as reported by a tool, before it becomes an API point.
#[derive(Debug, Clone, PartialEq)]
pub struct RawPoint {
    /// Tool-specific id part (`42`, `2024-01-01_10-00-01`, `root:5`, `2024-01-01-100000`).
    pub key: String,
    pub description: String,
    /// ISO-8601 date-time (with an offset when the tool reports one).
    pub created_at: Option<String>,
    /// Larger = newer, comparable within one tool (and one snapper config).
    pub order: i128,
    /// Extra text for the UI (tags, restore point type).
    pub note: Option<String>,
}

// ---------------------------------------------------------------- Windows

/// `20240101120000.000000-060` -> `2024-01-01T12:00:00-01:00`. The last part is the UTC offset in
/// minutes, east positive (WMI `CIM_DATETIME`).
pub fn wmi_date_to_iso(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() < 25 || !s.is_char_boundary(14) || !s[..14].bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<u32>().ok();
    let (y, mo, d, h, mi, se) = (n(0..4)?, n(4..6)?, n(6..8)?, n(8..10)?, n(10..12)?, n(12..14)?);
    let sign = match s.as_bytes().get(21)? {
        b'+' => 1i32,
        b'-' => -1i32,
        _ => return None,
    };
    let minutes: i32 = s.get(22..25)?.parse().ok()?;
    let off = UtcOffset::from_whole_seconds(sign * minutes * 60).ok()?;
    let date = Date::from_calendar_date(y as i32, Month::try_from(mo as u8).ok()?, d as u8).ok()?;
    let time = Time::from_hms(h as u8, mi as u8, se as u8).ok()?;
    PrimitiveDateTime::new(date, time)
        .assume_offset(off)
        .format(&Rfc3339)
        .ok()
}

/// PowerShell 5's `/Date(1700000000000)/` form.
fn ms_date_to_iso(s: &str) -> Option<String> {
    let inner = s.trim().strip_prefix("/Date(")?.strip_suffix(")/")?;
    let digits: String = inner.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
    let ms: i64 = digits.parse().ok()?;
    OffsetDateTime::from_unix_timestamp(ms / 1000).ok()?.format(&Rfc3339).ok()
}

fn restore_point_type(n: i64) -> Option<&'static str> {
    Some(match n {
        0 => "Program installed",
        1 => "Program removed",
        10 => "Driver installed",
        12 => "Settings changed",
        13 => "Cancelled operation",
        _ => return None,
    })
}

/// `Get-ComputerRestorePoint | Select ... | ConvertTo-Json`: `null`/empty, one object, or an array.
pub fn parse_windows_points(json: &str) -> Vec<RawPoint> {
    let t = json.trim().trim_start_matches('\u{feff}');
    if t.is_empty() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<Value>(t) else {
        return Vec::new();
    };
    let items: Vec<&Value> = match &v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![&v],
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for it in items {
        let Some(seq) = it
            .get("SequenceNumber")
            .and_then(|s| s.as_i64().or_else(|| s.as_str().and_then(|x| x.trim().parse().ok())))
        else {
            continue;
        };
        let created = it.get("CreationTime").and_then(|c| c.as_str()).and_then(|c| {
            wmi_date_to_iso(c).or_else(|| ms_date_to_iso(c))
        });
        out.push(RawPoint {
            key: seq.to_string(),
            description: it
                .get("Description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .trim()
                .to_string(),
            created_at: created,
            order: seq as i128,
            note: it
                .get("RestorePointType")
                .and_then(|t| t.as_i64())
                .and_then(restore_point_type)
                .map(str::to_string),
        });
    }
    out
}

// ---------------------------------------------------------------- Timeshift

fn is_timeshift_name(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() == 19
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 | 13 | 16 => *c == b'-',
            10 => *c == b'_',
            _ => c.is_ascii_digit(),
        })
}

/// `2024-01-01_10-00-01` -> (`2024-01-01T10:00:01`, 20240101100001)
fn timeshift_time(name: &str) -> (String, i128) {
    let iso = format!("{}T{}", &name[..10], name[11..].replace('-', ":"));
    let num: String = name.chars().filter(char::is_ascii_digit).collect();
    (iso, num.parse().unwrap_or(0))
}

/// `timeshift --list`: a table under a `Num Name Tags Description` header.
pub fn parse_timeshift_list(out: &str) -> Vec<RawPoint> {
    let mut v = Vec::new();
    for line in out.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(i) = tokens.iter().position(|t| is_timeshift_name(t)) else {
            continue;
        };
        // "0    >  2024-01-01_10-00-01  O  Before update": the row starts with a number
        if i == 0 || !tokens[0].bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let name = tokens[i];
        let mut rest = &tokens[i + 1..];
        let mut tags = None;
        if let Some(first) = rest.first() {
            if !first.is_empty() && first.bytes().all(|b| b"OBHDWM".contains(&b)) {
                tags = Some(*first);
                rest = &rest[1..];
            }
        }
        let (iso, order) = timeshift_time(name);
        let note = tags.map(|t| {
            t.chars()
                .map(|c| match c {
                    'O' => "on demand",
                    'B' => "boot",
                    'H' => "hourly",
                    'D' => "daily",
                    'W' => "weekly",
                    _ => "monthly",
                })
                .collect::<Vec<_>>()
                .join(", ")
        });
        v.push(RawPoint {
            key: name.to_string(),
            description: rest.join(" "),
            created_at: Some(iso),
            order,
            note,
        });
    }
    v
}

// ---------------------------------------------------------------- snapper

fn is_separator(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().all(|c| matches!(c, '-' | '+' | '=' | '|' | ' '))
}

fn cells(line: &str) -> Vec<&str> {
    line.split('|').map(str::trim).collect()
}

/// `snapper list-configs`: `Config | Subvolume` rows.
pub fn parse_snapper_configs(out: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_table = false;
    for line in out.lines() {
        if is_separator(line) {
            in_table = true;
            continue;
        }
        if !in_table || !line.contains('|') {
            continue;
        }
        let c = cells(line);
        if let Some(n) = c.first() {
            if !n.is_empty()
                && n.chars().all(|ch| ch.is_ascii_alphanumeric() || "_-.".contains(ch))
                && !n.starts_with('-')
            {
                names.push((*n).to_string());
            }
        }
    }
    names
}

/// `snapper --iso list`: skips the separator rows and snapshot 0 ("current").
pub fn parse_snapper_list(config: &str, out: &str) -> Vec<RawPoint> {
    let mut idx: Option<(usize, Option<usize>, Option<usize>, Option<usize>)> = None; // (#, date, description, type)
    let mut v = Vec::new();
    for line in out.lines() {
        if !line.contains('|') || is_separator(line) {
            continue;
        }
        let c = cells(line);
        if idx.is_none() {
            if c.first().is_some_and(|h| h.starts_with('#')) {
                let find = |name: &str| c.iter().position(|h| h.eq_ignore_ascii_case(name));
                idx = Some((0, find("Date"), find("Description"), find("Type")));
            }
            continue;
        }
        let (_, date_i, desc_i, type_i) = idx.unwrap();
        // "1*" (default snapshot), "3-" (active), "4+" : strip the marker
        let num_text = c[0].trim_end_matches(['*', '-', '+']).trim();
        let Ok(num) = num_text.parse::<u32>() else {
            continue;
        };
        if num == 0 {
            continue;
        }
        let date_text = date_i.and_then(|i| c.get(i)).copied().unwrap_or("");
        let created = iso_from_snapper_date(date_text);
        let ty = type_i.and_then(|i| c.get(i)).copied().unwrap_or("");
        v.push(RawPoint {
            key: format!("{config}:{num}"),
            description: desc_i.and_then(|i| c.get(i)).copied().unwrap_or("").to_string(),
            created_at: created,
            order: num as i128,
            note: (!ty.is_empty()).then(|| ty.to_string()),
        });
    }
    v
}

/// `2024-01-01 10:00:00` (from `--iso`) -> `2024-01-01T10:00:00`; other formats are locale
/// dependent and are not guessed.
fn iso_from_snapper_date(s: &str) -> Option<String> {
    let s = s.trim();
    let (d, t) = s.split_once(' ')?;
    let db = d.as_bytes();
    let tb = t.trim().as_bytes();
    let ok = db.len() == 10
        && db[4] == b'-'
        && db[7] == b'-'
        && tb.len() >= 8
        && tb[2] == b':'
        && tb[5] == b':'
        && d.bytes().enumerate().all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
        && t.trim()[..8].bytes().enumerate().all(|(i, b)| i == 2 || i == 5 || b.is_ascii_digit());
    ok.then(|| format!("{d}T{}", &t.trim()[..8]))
}

// ---------------------------------------------------------------- tmutil

/// `tmutil listlocalsnapshots /`: `com.apple.TimeMachine.2024-01-01-100000[.local]` per line.
pub fn parse_tmutil(out: &str) -> Vec<RawPoint> {
    let mut v = Vec::new();
    for line in out.lines() {
        let l = line.trim();
        let Some(rest) = l.strip_prefix("com.apple.TimeMachine.") else {
            continue;
        };
        let date = rest.strip_suffix(".local").unwrap_or(rest);
        let b = date.as_bytes();
        let ok = b.len() == 17
            && b.iter().enumerate().all(|(i, c)| match i {
                4 | 7 | 10 => *c == b'-',
                _ => c.is_ascii_digit(),
            });
        if !ok {
            continue;
        }
        let iso = format!("{}T{}:{}:{}", &date[..10], &date[11..13], &date[13..15], &date[15..17]);
        let num: String = date.chars().filter(char::is_ascii_digit).collect();
        v.push(RawPoint {
            key: date.to_string(),
            description: "Local Time Machine snapshot".to_string(),
            created_at: Some(iso),
            order: num.parse().unwrap_or(0),
            note: None,
        });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wmi_dates() {
        assert_eq!(
            wmi_date_to_iso("20240101120000.000000-000").as_deref(),
            Some("2024-01-01T12:00:00Z")
        );
        assert_eq!(
            wmi_date_to_iso("20240229235959.123456+060").as_deref(),
            Some("2024-02-29T23:59:59+01:00")
        );
        assert_eq!(
            wmi_date_to_iso("20240615083000.000000-480").as_deref(),
            Some("2024-06-15T08:30:00-08:00")
        );
        for bad in ["", "2024", "not a date at all....", "20241301120000.000000-000", "20240230120000.000000-000", "20240101250000.000000-000", "20240101120000.000000*000"] {
            assert_eq!(wmi_date_to_iso(bad), None, "{bad}");
        }
        assert_eq!(ms_date_to_iso("/Date(1700000000000)/").as_deref(), Some("2023-11-14T22:13:20Z"));
        assert_eq!(ms_date_to_iso("/Date(x)/"), None);
    }

    #[test]
    fn windows_json_array() {
        let json = r#"[
          {"SequenceNumber": 3, "Description": "Windows Update", "CreationTime": "20240105101500.000000-000", "RestorePointType": 0},
          {"SequenceNumber": 4, "Description": "Before ClearSweep", "CreationTime": "20240201093000.000000-000", "RestorePointType": 12}
        ]"#;
        let p = parse_windows_points(json);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].key, "3");
        assert_eq!(p[0].description, "Windows Update");
        assert_eq!(p[0].created_at.as_deref(), Some("2024-01-05T10:15:00Z"));
        assert_eq!(p[0].note.as_deref(), Some("Program installed"));
        assert_eq!(p[1].order, 4);
        assert_eq!(p[1].note.as_deref(), Some("Settings changed"));
    }

    #[test]
    fn windows_json_single_object_and_odd_inputs() {
        // ConvertTo-Json unwraps a one-element result
        let p = parse_windows_points(r#"{"SequenceNumber": 7, "Description": "Only one", "CreationTime": "20240301000000.000000-000", "RestorePointType": 13}"#);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].key, "7");
        assert_eq!(p[0].note.as_deref(), Some("Cancelled operation"));
        assert!(parse_windows_points("").is_empty());
        assert!(parse_windows_points("null").is_empty());
        assert!(parse_windows_points("[]").is_empty());
        assert!(parse_windows_points("not json").is_empty());
        assert!(parse_windows_points("\u{feff}[]").is_empty());
        // missing pieces are tolerated, an entry without a sequence number is dropped
        let p = parse_windows_points(r#"[{"SequenceNumber":"9"},{"Description":"no seq"},{"SequenceNumber":10,"CreationTime":"/Date(1700000000000)/"}]"#);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].key, "9");
        assert_eq!(p[0].description, "");
        assert_eq!(p[0].created_at, None);
        assert_eq!(p[1].created_at.as_deref(), Some("2023-11-14T22:13:20Z"));
    }

    const TIMESHIFT: &str = "Mounted '/dev/sda1' at '/run/timeshift/1234/backup'\nDevice : /dev/sda1\nUUID   : 0a1b2c3d\nPath   : /run/timeshift/1234/backup\nMode   : RSYNC\nDevice is OK\n2 snapshots, 35.8 GB free\n\nNum     Name                 Tags  Description\n------------------------------------------------------------------------------\n0    >  2024-01-01_10-00-01  O     Before the big update\n1    >  2024-02-01_03-00-00  D     \n2    >  2024-03-01_03-00-00        no tags here\n";

    #[test]
    fn timeshift_table() {
        let p = parse_timeshift_list(TIMESHIFT);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].key, "2024-01-01_10-00-01");
        assert_eq!(p[0].description, "Before the big update");
        assert_eq!(p[0].created_at.as_deref(), Some("2024-01-01T10:00:01"));
        assert_eq!(p[0].note.as_deref(), Some("on demand"));
        assert_eq!(p[1].note.as_deref(), Some("daily"));
        assert_eq!(p[1].description, "");
        assert_eq!(p[2].description, "no tags here");
        assert_eq!(p[2].note, None);
        assert!(p[0].order < p[1].order && p[1].order < p[2].order);
        assert!(parse_timeshift_list("").is_empty());
        assert!(parse_timeshift_list("No snapshots found\n").is_empty());
        assert!(parse_timeshift_list("Timeshift needs to be run as root\n").is_empty());
    }

    const SNAPPER: &str = "    # | Type   | Pre # | Date                | User | Cleanup  | Description | Userdata\n------+--------+-------+---------------------+------+----------+-------------+---------\n   0  | single |       |                     | root |          | current     |\n   1* | single |       | 2024-01-01 10:00:00 | root | number   | first root  |\n  12- | pre    |       | 2024-02-01 11:30:05 | root | number   | apt         | important=yes\n  13  | post   |    12 | 2024-02-01 11:31:00 | root | number   | apt         |\n";

    #[test]
    fn snapper_table() {
        let p = parse_snapper_list("root", SNAPPER);
        assert_eq!(p.len(), 3, "{p:?}"); // snapshot 0 is skipped
        assert_eq!(p[0].key, "root:1");
        assert_eq!(p[0].description, "first root");
        assert_eq!(p[0].created_at.as_deref(), Some("2024-01-01T10:00:00"));
        assert_eq!(p[0].note.as_deref(), Some("single"));
        assert_eq!(p[1].key, "root:12");
        assert_eq!(p[1].order, 12);
        assert_eq!(p[2].note.as_deref(), Some("post"));
        assert!(parse_snapper_list("root", "").is_empty());
        assert!(parse_snapper_list("root", "No permissions.\n").is_empty());
        // only separator rows and a header
        assert!(parse_snapper_list("root", "# | Type\n--+---\n").is_empty());
    }

    #[test]
    fn snapper_locale_dates_are_not_guessed() {
        let out = "# | Type | Pre # | Date | User | Cleanup | Description | Userdata\n--+------+-------+------+------+---------+-------------+---------\n5 | single | | Mon 01 Jan 2024 10:00:00 AM CET | root | | x |\n";
        let p = parse_snapper_list("home", out);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].created_at, None);
        assert_eq!(p[0].key, "home:5");
    }

    #[test]
    fn snapper_configs() {
        let out = "Config | Subvolume\n-------+----------\nroot   | /\nhome   | /home\n";
        assert_eq!(parse_snapper_configs(out), ["root", "home"]);
        assert!(parse_snapper_configs("").is_empty());
        assert!(parse_snapper_configs("Config | Subvolume\n-------+----------\n").is_empty());
        assert!(parse_snapper_configs("Failed to list configs\n").is_empty());
        assert!(parse_snapper_configs("-------+---\n--evil | /\n").is_empty());
    }

    #[test]
    fn tmutil_output() {
        let out = "Snapshots for volume group containing disk /:\ncom.apple.TimeMachine.2024-01-01-100000.local\ncom.apple.TimeMachine.2024-02-15-230501\ncom.apple.TimeMachine.garbage\nrandom line\n";
        let p = parse_tmutil(out);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].key, "2024-01-01-100000");
        assert_eq!(p[0].created_at.as_deref(), Some("2024-01-01T10:00:00"));
        assert_eq!(p[1].key, "2024-02-15-230501");
        assert!(p[0].order < p[1].order);
        assert!(parse_tmutil("").is_empty());
        assert!(parse_tmutil("Snapshots for volume group containing disk /:\n").is_empty());
    }
}
