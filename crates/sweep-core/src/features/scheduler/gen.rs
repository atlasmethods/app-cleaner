//! Pure generators for every OS scheduler format (no I/O; see `sync.rs` for the effects).

use crate::osjobs::{self, CalendarSlot, LaunchTrigger};

use super::model::{Frequency, Schedule};

const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

pub fn day_name(d: u8) -> &'static str {
    DAYS[usize::from(d.clamp(1, 7)) - 1]
}

// ---------------------------------------------------------------- names

pub fn systemd_unit_base(id: &str) -> String {
    format!("clearsweep-{id}")
}
pub fn systemd_timer_name(id: &str) -> String {
    format!("clearsweep-{id}.timer")
}
pub fn systemd_service_name(id: &str) -> String {
    format!("clearsweep-{id}.service")
}
pub fn xdg_login_file(id: &str) -> String {
    format!("clearsweep-schedule-{id}.desktop")
}
pub fn schtasks_name(id: &str) -> String {
    format!(r"ClearSweep\{id}")
}
pub fn win_run_value(id: &str) -> String {
    format!("ClearSweepSchedule-{id}")
}
pub fn launchd_label(id: &str) -> String {
    format!("app.clearsweep.schedule.{id}")
}
pub const CRON_TAG_PREFIX: &str = "# clearsweep-schedule:";
pub fn cron_tag(id: &str) -> String {
    format!("{CRON_TAG_PREFIX}{id}")
}

// ---------------------------------------------------------------- systemd

/// The `OnCalendar=` expression of `s` (`on_login` has none).
pub fn on_calendar(s: &Schedule) -> Option<String> {
    let (h, m) = s.hm();
    match s.frequency {
        Frequency::OnLogin => None,
        Frequency::Hourly if m == 0 => Some("hourly".to_string()),
        Frequency::Hourly => Some(format!("*-*-* *:{m:02}:00")),
        Frequency::Daily => Some(format!("*-*-* {h:02}:{m:02}:00")),
        Frequency::Weekly => {
            let days: Vec<&str> = s.weekdays.iter().map(|d| day_name(*d)).collect();
            Some(format!("{} *-*-* {h:02}:{m:02}:00", days.join(",")))
        }
        Frequency::Monthly => Some(format!("*-*-{:02} {h:02}:{m:02}:00", s.day_of_month)),
    }
}

pub fn systemd_service(s: &Schedule, exe: &str) -> String {
    let args = osjobs::schedule_args(&s.id);
    format!(
        "[Unit]\nDescription=ClearSweep scheduled clean ({})\n\n[Service]\nType=oneshot\nNice=10\nIOSchedulingClass=idle\nExecStart={}\n",
        osjobs::one_line(&s.name),
        osjobs::systemd_exec_start(exe, &args),
    )
}

/// `None` for `on_login` (handled by an autostart entry instead).
pub fn systemd_timer(s: &Schedule) -> Option<String> {
    let cal = on_calendar(s)?;
    Some(format!(
        "[Unit]\nDescription=ClearSweep schedule ({})\n\n[Timer]\nOnCalendar={cal}\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n",
        osjobs::one_line(&s.name),
    ))
}

// ---------------------------------------------------------------- XDG (on_login)

pub fn login_desktop_entry(s: &Schedule, exe: &str) -> String {
    osjobs::desktop_entry(
        &format!("ClearSweep scheduled clean ({})", s.name),
        "Cleans when you log in",
        exe,
        &osjobs::schedule_args(&s.id),
    )
}

// ---------------------------------------------------------------- cron

/// cron day-of-week: 0 = Sunday.
fn cron_dow(d: u8) -> u8 {
    d % 7
}

/// The crontab line for `s`, tagged so it can be found again. `None` when cron cannot express
/// the schedule (`on_login` is handled by an autostart entry).
pub fn cron_line(s: &Schedule, exe: &str) -> Option<String> {
    let (h, m) = s.hm();
    let when = match s.frequency {
        Frequency::OnLogin => return None,
        Frequency::Hourly => format!("{m} * * * *"),
        Frequency::Daily => format!("{m} {h} * * *"),
        Frequency::Weekly => {
            let mut d: Vec<u8> = s.weekdays.iter().map(|d| cron_dow(*d)).collect();
            d.sort_unstable();
            let list: Vec<String> = d.iter().map(u8::to_string).collect();
            format!("{m} {h} * * {}", list.join(","))
        }
        Frequency::Monthly => format!("{m} {h} {} * *", s.day_of_month),
    };
    let cmd = osjobs::cron_escape(&osjobs::sh_command(exe, &osjobs::schedule_args(&s.id)));
    Some(format!("{when} {cmd} {}", cron_tag(&s.id)))
}

fn is_tagged(line: &str, id: &str) -> bool {
    let tag = cron_tag(id);
    let t = line.trim_end();
    t.ends_with(&tag)
        && t[..t.len() - tag.len()]
            .chars()
            .last()
            .is_some_and(char::is_whitespace)
}

/// Does the crontab contain a line tagged for schedule `id`?
pub fn cron_has(text: &str, id: &str) -> bool {
    text.split_inclusive('\n').any(|l| is_tagged(l, id))
}

/// `text` with the lines tagged for `id` replaced by `new` (in place of the first one, else
/// appended) or, for `None`, removed. Every other line is kept byte for byte.
pub fn cron_upsert(text: &str, id: &str, new: Option<&str>) -> String {
    let mut out = String::with_capacity(text.len() + 128);
    let mut placed = false;
    for seg in text.split_inclusive('\n') {
        if is_tagged(seg, id) {
            if let (Some(line), false) = (new, placed) {
                out.push_str(line);
                out.push('\n');
                placed = true;
            }
            continue;
        }
        out.push_str(seg);
    }
    if let (Some(line), false) = (new, placed) {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------- Windows

/// Arguments of `schtasks` that create (or replace) the task for `s`.
pub fn schtasks_create_args(s: &Schedule, exe: &str) -> Vec<String> {
    let (h, m) = s.hm();
    let mut a: Vec<String> = ["/create", "/f", "/tn"]
        .into_iter()
        .map(String::from)
        .collect();
    a.push(schtasks_name(&s.id));
    a.push("/tr".to_string());
    a.push(osjobs::windows_command(exe, &osjobs::schedule_args(&s.id)));
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|x| x.to_string()));
    match s.frequency {
        Frequency::Daily => push(&["/sc", "DAILY", "/st", &format!("{h:02}:{m:02}")]),
        Frequency::Weekly => {
            let days: Vec<String> = s
                .weekdays
                .iter()
                .map(|d| day_name(*d).to_uppercase())
                .collect();
            push(&[
                "/sc",
                "WEEKLY",
                "/d",
                &days.join(","),
                "/st",
                &format!("{h:02}:{m:02}"),
            ]);
        }
        Frequency::Monthly => push(&[
            "/sc",
            "MONTHLY",
            "/d",
            &s.day_of_month.to_string(),
            "/st",
            &format!("{h:02}:{m:02}"),
        ]),
        Frequency::Hourly => push(&["/sc", "HOURLY", "/st", &format!("00:{m:02}")]),
        Frequency::OnLogin => push(&["/sc", "ONLOGON"]),
    }
    a
}

pub fn schtasks_change_args(id: &str, enabled: bool) -> Vec<String> {
    vec![
        "/change".to_string(),
        "/tn".to_string(),
        schtasks_name(id),
        if enabled { "/enable" } else { "/disable" }.to_string(),
    ]
}

pub fn schtasks_delete_args(id: &str) -> Vec<String> {
    vec![
        "/delete".to_string(),
        "/tn".to_string(),
        schtasks_name(id),
        "/f".to_string(),
    ]
}

// ---------------------------------------------------------------- launchd

pub fn launchd_slots(s: &Schedule) -> Vec<CalendarSlot> {
    let (h, m) = s.hm();
    match s.frequency {
        Frequency::OnLogin => Vec::new(),
        Frequency::Hourly => vec![CalendarSlot {
            minute: Some(m),
            ..Default::default()
        }],
        Frequency::Daily => vec![CalendarSlot {
            minute: Some(m),
            hour: Some(h),
            ..Default::default()
        }],
        Frequency::Weekly => s
            .weekdays
            .iter()
            .map(|d| CalendarSlot {
                minute: Some(m),
                hour: Some(h),
                weekday: Some(*d),
                ..Default::default()
            })
            .collect(),
        Frequency::Monthly => vec![CalendarSlot {
            minute: Some(m),
            hour: Some(h),
            day: Some(s.day_of_month),
            ..Default::default()
        }],
    }
}

pub fn launchd_plist(s: &Schedule, exe: &str) -> String {
    let trigger = if s.frequency == Frequency::OnLogin {
        LaunchTrigger::RunAtLoad
    } else {
        LaunchTrigger::Calendar(launchd_slots(s))
    };
    osjobs::launchd_plist(
        &launchd_label(&s.id),
        exe,
        &osjobs::schedule_args(&s.id),
        &trigger,
    )
}
