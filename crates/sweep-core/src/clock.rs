//! Time source behind a trait (so the background agent is testable without sleeping) plus the
//! little calendar arithmetic the scheduler needs.

use std::time::{SystemTime, UNIX_EPOCH};

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Wall-clock time in the user's time zone, to the minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1..=12
    pub month: u8,
    /// 1..=31
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

impl LocalTime {
    pub fn new(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> Self {
        Self {
            year,
            month,
            day,
            hour,
            minute,
        }
    }

    /// Minutes since 1970-01-01 00:00 *local* time (not UTC): only differences are meaningful.
    pub fn local_minutes(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * 1440
            + i64::from(self.hour) * 60
            + i64::from(self.minute)
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i32, m: u8, d: u8) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`]: `(year, month 1..=12, day 1..=31)`.
pub fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    ((y + i64::from(m <= 2)) as i32, m as u8, d as u8)
}

/// ISO weekday of a day number from [`days_from_civil`]: 1 = Monday .. 7 = Sunday.
pub fn iso_weekday(days: i64) -> u8 {
    // 1970-01-01 was a Thursday (4).
    ((days + 3).rem_euclid(7) + 1) as u8
}

/// Milliseconds since the Unix epoch as RFC 3339 (UTC).
pub fn rfc3339_from_ms(ms: u64) -> String {
    OffsetDateTime::from_unix_timestamp((ms / 1000) as i64)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

/// RFC 3339 -> milliseconds since the Unix epoch.
pub fn ms_from_rfc3339(s: &str) -> Option<u64> {
    let t = OffsetDateTime::parse(s, &Rfc3339).ok()?;
    u64::try_from(t.unix_timestamp()).ok().map(|s| s * 1000)
}

pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> u64;
    /// The current local wall-clock time, when the OS can tell.
    fn local(&self) -> Option<LocalTime>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn local(&self) -> Option<LocalTime> {
        local_now()
    }
}

#[cfg(unix)]
fn local_now() -> Option<LocalTime> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as libc::time_t;
    // SAFETY: `tm` is plain data that localtime_r fully initialises on success; both
    // pointers are valid for the duration of the call.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return None;
        }
        tm
    };
    Some(LocalTime::new(
        tm.tm_year + 1900,
        (tm.tm_mon + 1) as u8,
        tm.tm_mday as u8,
        tm.tm_hour as u8,
        tm.tm_min as u8,
    ))
}

#[cfg(windows)]
fn local_now() -> Option<LocalTime> {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: GetLocalTime writes a SYSTEMTIME through the pointer and cannot fail.
    let st = unsafe {
        let mut st = std::mem::zeroed();
        GetLocalTime(&mut st);
        st
    };
    Some(LocalTime::new(
        i32::from(st.wYear),
        st.wMonth as u8,
        st.wDay as u8,
        st.wHour as u8,
        st.wMinute as u8,
    ))
}

#[cfg(not(any(unix, windows)))]
fn local_now() -> Option<LocalTime> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for days in [
            -800_000i64,
            -1,
            0,
            1,
            59,
            60,
            365,
            11_016,
            19_000,
            20_000,
            2_932_896,
        ] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn weekdays() {
        assert_eq!(iso_weekday(days_from_civil(1970, 1, 1)), 4); // Thursday
        assert_eq!(iso_weekday(days_from_civil(2024, 1, 1)), 1); // Monday
        assert_eq!(iso_weekday(days_from_civil(2024, 1, 7)), 7); // Sunday
        assert_eq!(iso_weekday(days_from_civil(1969, 12, 31)), 3); // Wednesday
    }

    #[test]
    fn rfc3339_round_trip() {
        let s = rfc3339_from_ms(1_700_000_000_000);
        assert_eq!(s, "2023-11-14T22:13:20Z");
        assert_eq!(ms_from_rfc3339(&s), Some(1_700_000_000_000));
        assert_eq!(ms_from_rfc3339("nonsense"), None);
    }

    #[test]
    fn local_minutes_differences() {
        let a = LocalTime::new(2024, 3, 1, 0, 0);
        let b = LocalTime::new(2024, 2, 29, 23, 30);
        assert_eq!(a.local_minutes() - b.local_minutes(), 30);
    }

    #[test]
    fn system_clock_is_sane() {
        assert!(SystemClock.now_ms() > 1_600_000_000_000);
        let l = SystemClock.local().expect("local time");
        assert!((1..=12).contains(&l.month) && (1..=31).contains(&l.day));
    }
}
