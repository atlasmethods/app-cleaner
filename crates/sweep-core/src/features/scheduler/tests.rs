//! Scheduler: generated text (every OS format), OS sync command sequences, validation, the
//! API round trip and the missed-run catch-up.

use super::gen::*;
use super::model::*;
use super::*;
use crate::api::dispatch;
use crate::clock::{Clock, LocalTime};
use crate::ctx::Os;
use crate::fakesys::FakeSys;
use crate::features::cleaner::history;
use crate::testutil::{Chromium, Fixture};
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const ID: &str = "0123456789abcdef";
const EXE: &str = "/usr/bin/clearsweep";

fn sched(freq: Frequency, time: &str, weekdays: &[u8], dom: u8) -> Schedule {
    Schedule {
        id: ID.to_string(),
        name: "Nightly".to_string(),
        enabled: true,
        frequency: freq,
        time: time.to_string(),
        weekdays: weekdays.to_vec(),
        day_of_month: dom,
        action: Action::default(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        last_run: None,
        last_result: None,
    }
}

// ---------------------------------------------------------------- systemd text

#[test]
fn on_calendar_for_every_frequency() {
    let cases = [
        (
            sched(Frequency::Daily, "03:00", &[], 1),
            Some("*-*-* 03:00:00"),
        ),
        (
            sched(Frequency::Daily, "23:59", &[], 1),
            Some("*-*-* 23:59:00"),
        ),
        (
            sched(Frequency::Weekly, "03:00", &[1, 3], 1),
            Some("Mon,Wed *-*-* 03:00:00"),
        ),
        (
            sched(Frequency::Weekly, "07:05", &[7], 1),
            Some("Sun *-*-* 07:05:00"),
        ),
        (
            sched(Frequency::Weekly, "00:00", &[1, 2, 3, 4, 5, 6, 7], 1),
            Some("Mon,Tue,Wed,Thu,Fri,Sat,Sun *-*-* 00:00:00"),
        ),
        (
            sched(Frequency::Monthly, "03:00", &[], 15),
            Some("*-*-15 03:00:00"),
        ),
        (
            sched(Frequency::Monthly, "04:30", &[], 1),
            Some("*-*-01 04:30:00"),
        ),
        (
            sched(Frequency::Monthly, "04:30", &[], 28),
            Some("*-*-28 04:30:00"),
        ),
        (sched(Frequency::Hourly, "00:00", &[], 1), Some("hourly")),
        (sched(Frequency::Hourly, "05:00", &[], 1), Some("hourly")),
        (
            sched(Frequency::Hourly, "00:15", &[], 1),
            Some("*-*-* *:15:00"),
        ),
        (sched(Frequency::OnLogin, "03:00", &[], 1), None),
    ];
    for (s, want) in cases {
        assert_eq!(
            on_calendar(&s).as_deref(),
            want,
            "{:?} {}",
            s.frequency,
            s.time
        );
    }
}

#[test]
fn systemd_unit_files_are_exact() {
    let s = sched(Frequency::Weekly, "03:00", &[1, 3], 1);
    assert_eq!(
        systemd_service(&s, EXE),
        "[Unit]\nDescription=ClearSweep scheduled clean (Nightly)\n\n[Service]\nType=oneshot\nNice=10\nIOSchedulingClass=idle\nExecStart=\"/usr/bin/clearsweep\" \"clean\" \"--auto\" \"--source\" \"scheduled\" \"--schedule\" \"0123456789abcdef\"\n"
    );
    assert_eq!(
        systemd_timer(&s).unwrap(),
        "[Unit]\nDescription=ClearSweep schedule (Nightly)\n\n[Timer]\nOnCalendar=Mon,Wed *-*-* 03:00:00\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n"
    );
    assert!(systemd_timer(&sched(Frequency::OnLogin, "03:00", &[], 1)).is_none());
}

#[test]
fn systemd_unit_escapes_hostile_paths_and_names() {
    let mut s = sched(Frequency::Daily, "03:00", &[], 1);
    s.name = "50% clean\nExecStart=/bin/evil".to_string();
    let svc = systemd_service(&s, "/opt/My \"App\" $HOME/clearsweep");
    assert!(
        svc.contains("ExecStart=\"/opt/My \\\"App\\\" $$HOME/clearsweep\" \"clean\""),
        "{svc}"
    );
    // The newline in the name cannot start a new directive.
    assert_eq!(svc.matches("\nExecStart=").count(), 1, "{svc}");
    assert!(svc.contains("Description=ClearSweep scheduled clean (50%% clean ExecStart=/bin/evil)"));
}

// ---------------------------------------------------------------- cron text

#[test]
fn cron_lines_for_every_frequency() {
    let tag = format!("# clearsweep-schedule:{ID}");
    let cmd = format!("{EXE} clean --auto --source scheduled --schedule {ID}");
    let cases = [
        (
            sched(Frequency::Daily, "03:00", &[], 1),
            format!("0 3 * * * {cmd} {tag}"),
        ),
        (
            sched(Frequency::Daily, "23:59", &[], 1),
            format!("59 23 * * * {cmd} {tag}"),
        ),
        (
            sched(Frequency::Weekly, "03:00", &[1, 3], 1),
            format!("0 3 * * 1,3 {cmd} {tag}"),
        ),
        // cron counts Sunday as 0.
        (
            sched(Frequency::Weekly, "07:05", &[7, 1], 1),
            format!("5 7 * * 0,1 {cmd} {tag}"),
        ),
        (
            sched(Frequency::Monthly, "03:00", &[], 15),
            format!("0 3 15 * * {cmd} {tag}"),
        ),
        (
            sched(Frequency::Hourly, "00:30", &[], 1),
            format!("30 * * * * {cmd} {tag}"),
        ),
    ];
    for (s, want) in cases {
        assert_eq!(cron_line(&s, EXE).as_deref(), Some(want.as_str()));
    }
    assert!(cron_line(&sched(Frequency::OnLogin, "03:00", &[], 1), EXE).is_none());
}

#[test]
fn cron_line_quotes_the_command_and_escapes_percent() {
    let s = sched(Frequency::Daily, "03:00", &[], 1);
    let l = cron_line(&s, "/opt/My App/100%/clearsweep").unwrap();
    assert!(
        l.starts_with("0 3 * * * '/opt/My App/100\\%/clearsweep' clean --auto"),
        "{l}"
    );
    assert!(!l.contains("100%/"), "a bare % would end the command: {l}");
}

#[test]
fn cron_upsert_preserves_every_other_line_exactly() {
    let other = format!("# clearsweep-schedule:{}", "ffffffffffffffff");
    let text = format!(
        "# my jobs\n*/5 * * * * /usr/bin/true\n\n@reboot /home/u/bin/sync.sh --quiet   \nMAILTO=me@example.org\n0 1 * * * echo hi # not ours\n5 5 * * * /bin/x {other}\n"
    );
    let line = cron_line(&sched(Frequency::Daily, "03:00", &[], 1), EXE).unwrap();
    let added = cron_upsert(&text, ID, Some(&line));
    assert_eq!(added, format!("{text}{line}\n"));
    // Idempotent.
    assert_eq!(cron_upsert(&added, ID, Some(&line)), added);
    // Replaced in place, not moved.
    let moved = format!("{}\n# tail\n", added.trim_end());
    let line2 = cron_line(&sched(Frequency::Daily, "04:00", &[], 1), EXE).unwrap();
    let replaced = cron_upsert(&moved, ID, Some(&line2));
    assert_eq!(replaced, format!("{text}{line2}\n# tail\n"));
    // Removal restores the original text byte for byte.
    assert_eq!(cron_upsert(&added, ID, None), text);
    assert_eq!(
        cron_upsert(&text, ID, None),
        text,
        "removing a missing schedule changes nothing"
    );
    // Another schedule's line, even with a lookalike id, is never touched.
    assert!(cron_upsert(&added, ID, None).contains(&other));
}

#[test]
fn cron_upsert_edge_cases() {
    let line = "0 3 * * * x # clearsweep-schedule:0123456789abcdef";
    assert_eq!(cron_upsert("", ID, Some(line)), format!("{line}\n"));
    // A crontab that does not end with a newline gets one before ours.
    assert_eq!(
        cron_upsert("@daily a", ID, Some(line)),
        format!("@daily a\n{line}\n")
    );
    // Duplicated tagged lines collapse into one.
    let dup = format!("{line}\nkeep\n{line}\n");
    assert_eq!(cron_upsert(&dup, ID, Some(line)), format!("{line}\nkeep\n"));
    assert_eq!(cron_upsert(&dup, ID, None), "keep\n");
    // The tag must be a whole token at the end of the line.
    let near = "0 3 * * * x#clearsweep-schedule:0123456789abcdef\n0 3 * * * y # clearsweep-schedule:0123456789abcdeff\n";
    assert_eq!(cron_upsert(near, ID, None), near);
    assert!(!cron_has(near, ID));
    assert!(cron_has(&dup, ID));
    // CRLF line endings survive on other lines.
    assert_eq!(
        cron_upsert("a\r\nb\r\n", ID, Some(line)),
        format!("a\r\nb\r\n{line}\n")
    );
}

// ---------------------------------------------------------------- schtasks

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn schtasks_arguments_for_every_frequency() {
    let exe = r"C:\Program Files\ClearSweep\clearsweep.exe";
    let tr = format!("\"{exe}\" clean --auto --source scheduled --schedule {ID}");
    let base = |extra: &[&str]| {
        let mut v = args(&[
            "/create",
            "/f",
            "/tn",
            &format!(r"ClearSweep\{ID}"),
            "/tr",
            &tr,
        ]);
        v.extend(args(extra));
        v
    };
    assert_eq!(
        schtasks_create_args(&sched(Frequency::Daily, "03:00", &[], 1), exe),
        base(&["/sc", "DAILY", "/st", "03:00"])
    );
    assert_eq!(
        schtasks_create_args(&sched(Frequency::Weekly, "03:00", &[1, 3], 1), exe),
        base(&["/sc", "WEEKLY", "/d", "MON,WED", "/st", "03:00"])
    );
    assert_eq!(
        schtasks_create_args(&sched(Frequency::Weekly, "21:45", &[7], 1), exe),
        base(&["/sc", "WEEKLY", "/d", "SUN", "/st", "21:45"])
    );
    assert_eq!(
        schtasks_create_args(&sched(Frequency::Monthly, "03:00", &[], 15), exe),
        base(&["/sc", "MONTHLY", "/d", "15", "/st", "03:00"])
    );
    assert_eq!(
        schtasks_create_args(&sched(Frequency::Hourly, "09:20", &[], 1), exe),
        base(&["/sc", "HOURLY", "/st", "00:20"])
    );
    assert_eq!(
        schtasks_create_args(&sched(Frequency::OnLogin, "03:00", &[], 1), exe),
        base(&["/sc", "ONLOGON"])
    );
    assert_eq!(
        schtasks_change_args(ID, false),
        args(&["/change", "/tn", &format!(r"ClearSweep\{ID}"), "/disable"])
    );
    assert_eq!(
        schtasks_change_args(ID, true),
        args(&["/change", "/tn", &format!(r"ClearSweep\{ID}"), "/enable"])
    );
    assert_eq!(
        schtasks_delete_args(ID),
        args(&["/delete", "/tn", &format!(r"ClearSweep\{ID}"), "/f"])
    );
}

// ---------------------------------------------------------------- launchd

#[test]
fn launchd_plists_for_every_frequency() {
    let parse = |s: &Schedule| -> plist::Dictionary {
        let t = launchd_plist(s, "/Applications/ClearSweep.app/Contents/MacOS/clearsweep");
        plist::Value::from_reader_xml(t.as_bytes())
            .expect("valid plist")
            .into_dictionary()
            .unwrap()
    };
    let int = |d: &plist::Dictionary, k: &str| d.get(k).and_then(|v| v.as_signed_integer());

    let d = parse(&sched(Frequency::Daily, "03:30", &[], 1));
    assert_eq!(
        d.get("Label").unwrap().as_string(),
        Some("app.clearsweep.schedule.0123456789abcdef")
    );
    let pa: Vec<&str> = d
        .get("ProgramArguments")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_string().unwrap())
        .collect();
    assert_eq!(
        pa,
        [
            "/Applications/ClearSweep.app/Contents/MacOS/clearsweep",
            "clean",
            "--auto",
            "--source",
            "scheduled",
            "--schedule",
            ID
        ]
    );
    assert_eq!(d.get("RunAtLoad").unwrap().as_boolean(), Some(false));
    let cal = d
        .get("StartCalendarInterval")
        .unwrap()
        .as_dictionary()
        .unwrap();
    assert_eq!((int(cal, "Hour"), int(cal, "Minute")), (Some(3), Some(30)));
    assert!(cal.get("Weekday").is_none() && cal.get("Day").is_none());

    // Weekly: one dict per weekday.
    let d = parse(&sched(Frequency::Weekly, "03:00", &[1, 3, 7], 1));
    let arr = d.get("StartCalendarInterval").unwrap().as_array().unwrap();
    let days: Vec<i64> = arr
        .iter()
        .map(|v| int(v.as_dictionary().unwrap(), "Weekday").unwrap())
        .collect();
    assert_eq!(days, [1, 3, 7]);

    let d = parse(&sched(Frequency::Monthly, "04:00", &[], 15));
    let cal = d
        .get("StartCalendarInterval")
        .unwrap()
        .as_dictionary()
        .unwrap();
    assert_eq!(
        (int(cal, "Day"), int(cal, "Hour"), int(cal, "Minute")),
        (Some(15), Some(4), Some(0))
    );

    let d = parse(&sched(Frequency::Hourly, "00:15", &[], 1));
    let cal = d
        .get("StartCalendarInterval")
        .unwrap()
        .as_dictionary()
        .unwrap();
    assert_eq!(int(cal, "Minute"), Some(15));
    assert!(cal.get("Hour").is_none());

    let d = parse(&sched(Frequency::OnLogin, "03:00", &[], 1));
    assert_eq!(d.get("RunAtLoad").unwrap().as_boolean(), Some(true));
    assert!(d.get("StartCalendarInterval").is_none());
}

#[test]
fn login_desktop_entry_runs_the_scheduled_clean() {
    let s = sched(Frequency::OnLogin, "03:00", &[], 1);
    assert_eq!(
        login_desktop_entry(&s, EXE),
        format!("[Desktop Entry]\nType=Application\nName=ClearSweep scheduled clean (Nightly)\nComment=Cleans when you log in\nExec=/usr/bin/clearsweep clean --auto --source scheduled --schedule {ID}\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n")
    );
}

// ---------------------------------------------------------------- validation

fn known() -> Vec<String> {
    vec!["chrome.cache".to_string(), "temp.files".to_string()]
}

fn check(mut s: Schedule, others: &[Schedule]) -> std::result::Result<Schedule, String> {
    s.validate(others, &known())
        .map(|_| s)
        .map_err(|e| e.message)
}

#[test]
fn validation_accepts_and_normalises() {
    let mut s = sched(Frequency::Weekly, "03:00", &[3, 1, 3], 1);
    s.name = "  Nightly  ".into();
    s.action.rules = Some(vec!["chrome.cache".into(), "chrome.cache".into()]);
    let s = check(s, &[]).unwrap();
    assert_eq!(s.name, "Nightly");
    assert_eq!(s.weekdays, [1, 3]);
    assert_eq!(s.action.rules, Some(vec!["chrome.cache".to_string()]));
    assert!(check(sched(Frequency::Daily, "00:00", &[], 28), &[]).is_ok());
    assert!(check(sched(Frequency::Daily, "23:59", &[], 1), &[]).is_ok());
    // Weekdays are only required for weekly schedules.
    assert!(check(sched(Frequency::Monthly, "03:00", &[], 5), &[]).is_ok());
}

#[test]
fn validation_errors() {
    let bad = |s: Schedule, others: &[Schedule], needle: &str| {
        let e = check(s, others).unwrap_err();
        assert!(e.contains(needle), "{e:?} should mention {needle:?}");
    };
    for t in [
        "3:00", "24:00", "03:60", "0300", "03-00", "ab:cd", "03:0", "", "03:00:00", " 03:00",
    ] {
        bad(sched(Frequency::Daily, t, &[], 1), &[], "time");
    }
    bad(sched(Frequency::Weekly, "03:00", &[], 1), &[], "weekday");
    bad(sched(Frequency::Weekly, "03:00", &[0], 1), &[], "weekdays");
    bad(sched(Frequency::Weekly, "03:00", &[8], 1), &[], "weekdays");
    bad(
        sched(Frequency::Monthly, "03:00", &[], 0),
        &[],
        "dayOfMonth",
    );
    bad(
        sched(Frequency::Monthly, "03:00", &[], 29),
        &[],
        "dayOfMonth",
    );
    let mut s = sched(Frequency::Daily, "03:00", &[], 1);
    s.name = "   ".into();
    bad(s.clone(), &[], "name");
    s.name = "x".repeat(61);
    bad(s.clone(), &[], "name");
    s.name = "tab\there".into();
    bad(s.clone(), &[], "control");
    s.name = "nightly".into();
    let mut other = sched(Frequency::Daily, "05:00", &[], 1);
    other.id = "fedcba9876543210".into();
    other.name = "NIGHTLY".into();
    bad(s.clone(), &[other.clone()], "already a schedule named");
    // Renaming a schedule to its own name is fine.
    other.id = ID.into();
    assert!(check(s.clone(), &[other]).is_ok());
    s.action.rules = Some(vec![]);
    bad(s.clone(), &[], "at least one rule");
    s.action.rules = Some(vec!["nope.rule".into()]);
    bad(s.clone(), &[], "unknown rule `nope.rule`");
    s.id = "../../etc/passwd".into();
    s.action.rules = None;
    bad(s, &[], "invalid schedule id");
}

#[test]
fn ids_are_strict() {
    assert!(valid_id(ID));
    for bad in [
        "",
        "0123456789ABCDEF",
        "0123456789abcde",
        "0123456789abcdef0",
        "012345678 abcdef",
        "0123456789abcdeg",
    ] {
        assert!(!valid_id(bad), "{bad}");
    }
}

// ---------------------------------------------------------------- previous due

fn lt(y: i32, mo: u8, d: u8, h: u8, mi: u8) -> LocalTime {
    LocalTime::new(y, mo, d, h, mi)
}

fn due(s: &Schedule, now: LocalTime) -> Option<LocalTime> {
    previous_due_minutes(s, &now).map(|m| {
        let days = m.div_euclid(1440);
        let (y, mo, d) = crate::clock::civil_from_days(days);
        let rem = m.rem_euclid(1440);
        lt(y, mo, d, (rem / 60) as u8, (rem % 60) as u8)
    })
}

#[test]
fn previous_due_times() {
    // 2024-01-03 is a Wednesday.
    let daily = sched(Frequency::Daily, "03:00", &[], 1);
    assert_eq!(
        due(&daily, lt(2024, 1, 3, 9, 0)),
        Some(lt(2024, 1, 3, 3, 0))
    );
    assert_eq!(
        due(&daily, lt(2024, 1, 3, 3, 0)),
        Some(lt(2024, 1, 3, 3, 0))
    );
    assert_eq!(
        due(&daily, lt(2024, 1, 3, 2, 59)),
        Some(lt(2024, 1, 2, 3, 0))
    );
    assert_eq!(
        due(&daily, lt(2024, 3, 1, 1, 0)),
        Some(lt(2024, 2, 29, 3, 0)),
        "leap day"
    );

    let hourly = sched(Frequency::Hourly, "00:20", &[], 1);
    assert_eq!(
        due(&hourly, lt(2024, 1, 3, 9, 45)),
        Some(lt(2024, 1, 3, 9, 20))
    );
    assert_eq!(
        due(&hourly, lt(2024, 1, 3, 9, 10)),
        Some(lt(2024, 1, 3, 8, 20))
    );
    assert_eq!(
        due(&hourly, lt(2024, 1, 1, 0, 10)),
        Some(lt(2023, 12, 31, 23, 20))
    );

    let weekly = sched(Frequency::Weekly, "03:00", &[1, 3], 1); // Mon, Wed
    assert_eq!(
        due(&weekly, lt(2024, 1, 3, 9, 0)),
        Some(lt(2024, 1, 3, 3, 0))
    );
    assert_eq!(
        due(&weekly, lt(2024, 1, 3, 2, 0)),
        Some(lt(2024, 1, 1, 3, 0)),
        "Monday before"
    );
    assert_eq!(
        due(&weekly, lt(2024, 1, 6, 12, 0)),
        Some(lt(2024, 1, 3, 3, 0)),
        "Saturday"
    );
    let sunday = sched(Frequency::Weekly, "03:00", &[7], 1);
    assert_eq!(
        due(&sunday, lt(2024, 1, 3, 9, 0)),
        Some(lt(2023, 12, 31, 3, 0))
    );

    let monthly = sched(Frequency::Monthly, "03:00", &[], 15);
    assert_eq!(
        due(&monthly, lt(2024, 1, 3, 9, 0)),
        Some(lt(2023, 12, 15, 3, 0))
    );
    assert_eq!(
        due(&monthly, lt(2024, 1, 15, 3, 0)),
        Some(lt(2024, 1, 15, 3, 0))
    );
    assert_eq!(
        due(&monthly, lt(2024, 3, 1, 0, 0)),
        Some(lt(2024, 2, 15, 3, 0))
    );
    let first = sched(Frequency::Monthly, "03:00", &[], 1);
    assert_eq!(
        due(&first, lt(2024, 3, 31, 0, 0)),
        Some(lt(2024, 3, 1, 3, 0))
    );

    assert_eq!(
        due(
            &sched(Frequency::OnLogin, "03:00", &[], 1),
            lt(2024, 1, 3, 9, 0)
        ),
        None
    );
}

// ---------------------------------------------------------------- sync (command sequences)

struct Bed {
    _tmp: tempfile::TempDir,
    fx: Fixture,
    ctx: Ctx,
    sys: FakeSys,
}

fn bed(os: Os, programs: &[&str]) -> Bed {
    let tmp = tempfile::tempdir().unwrap();
    let mut fx = Fixture::new(tmp.path());
    fx.env.os = os;
    let sys = FakeSys::new(programs);
    let ctx = Ctx::new(fx.env.clone(), Arc::new(sys.clone()))
        .with_procs(Arc::new(crate::procs::FakeProcesses::new(&[], false)));
    Bed {
        _tmp: tmp,
        fx,
        ctx,
        sys,
    }
}

fn exe() -> &'static Path {
    Path::new(EXE)
}

fn unit_dir(b: &Bed) -> std::path::PathBuf {
    b.fx.env.config_dir.join("systemd/user")
}

#[test]
fn backend_detection_on_linux() {
    let b = bed(Os::Linux, &["systemctl", "crontab"]);
    let be = sync::detect(&b.ctx);
    assert_eq!((be.kind, be.available), (sync::BackendKind::Systemd, true));

    // systemctl exists but there is no user manager (container, ssh without a session).
    let b = bed(Os::Linux, &["systemctl", "crontab"]);
    b.sys.state.lock().unwrap().systemd_user = false;
    let be = sync::detect(&b.ctx);
    assert_eq!((be.kind, be.available), (sync::BackendKind::Cron, true));

    let b = bed(Os::Linux, &["systemctl"]);
    b.sys.state.lock().unwrap().systemd_user = false;
    let be = sync::detect(&b.ctx);
    assert_eq!((be.kind, be.available), (sync::BackendKind::None, false));
    assert!(be.detail.contains("agent"), "{}", be.detail);

    let b = bed(Os::Linux, &[]);
    assert!(!sync::detect(&b.ctx).available);
    assert_eq!(
        serde_json::to_value(sync::detect(&b.ctx)).unwrap()["kind"],
        "none"
    );
}

#[test]
fn backend_detection_elsewhere() {
    assert_eq!(
        sync::detect(&bed(Os::Windows, &["schtasks"]).ctx).kind,
        sync::BackendKind::Schtasks
    );
    assert!(!sync::detect(&bed(Os::Windows, &[]).ctx).available);
    assert_eq!(
        sync::detect(&bed(Os::MacOs, &["launchctl"]).ctx).kind,
        sync::BackendKind::Launchd
    );
    assert!(!sync::detect(&bed(Os::MacOs, &[]).ctx).available);
}

#[test]
fn systemd_install_update_disable_remove() {
    let b = bed(Os::Linux, &["systemctl", "crontab"]);
    let mut s = sched(Frequency::Daily, "03:00", &[], 1);
    let timer = unit_dir(&b).join(format!("clearsweep-{ID}.timer"));
    let service = unit_dir(&b).join(format!("clearsweep-{ID}.service"));

    sync::sync(&b.ctx, exe(), &s).unwrap();
    assert_eq!(
        b.sys.calls(),
        [
            "systemctl --user show-environment",
            "systemctl --user daemon-reload",
            &format!("systemctl --user enable --now clearsweep-{ID}.timer"),
        ]
    );
    assert_eq!(
        std::fs::read_to_string(&service).unwrap(),
        systemd_service(&s, EXE)
    );
    assert_eq!(
        std::fs::read_to_string(&timer).unwrap(),
        systemd_timer(&s).unwrap()
    );
    assert!(std::fs::read_to_string(&timer)
        .unwrap()
        .contains("OnCalendar=*-*-* 03:00:00"));

    // Same schedule again: no restart. A changed time restarts the running timer.
    b.sys.clear_calls();
    sync::sync(&b.ctx, exe(), &s).unwrap();
    assert!(!b.sys.calls().iter().any(|c| c.contains("restart")));
    b.sys.clear_calls();
    s.time = "04:15".into();
    sync::sync(&b.ctx, exe(), &s).unwrap();
    let calls = b.sys.calls();
    assert_eq!(
        calls.last().unwrap(),
        &format!("systemctl --user restart clearsweep-{ID}.timer")
    );
    assert!(std::fs::read_to_string(&timer)
        .unwrap()
        .contains("*-*-* 04:15:00"));

    // Disabled: files stay, the timer is switched off.
    b.sys.clear_calls();
    s.enabled = false;
    sync::sync(&b.ctx, exe(), &s).unwrap();
    assert_eq!(
        b.sys.calls(),
        [
            "systemctl --user show-environment",
            "systemctl --user daemon-reload",
            &format!("systemctl --user disable --now clearsweep-{ID}.timer"),
        ]
    );
    assert!(timer.exists() && service.exists());

    // Removal deletes everything and reloads.
    b.sys.clear_calls();
    let warnings = sync::remove(&b.ctx, ID);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(!timer.exists() && !service.exists());
    assert_eq!(
        b.sys.calls_of("systemctl"),
        [
            format!("--user disable --now clearsweep-{ID}.timer"),
            "--user daemon-reload".to_string()
        ]
    );
    // Removing again does nothing (and runs nothing systemd-related).
    b.sys.clear_calls();
    assert!(sync::remove(&b.ctx, ID).is_empty());
    assert!(b.sys.calls_of("systemctl").is_empty());
}

#[test]
fn systemd_failures_are_reported() {
    let b = bed(Os::Linux, &["systemctl"]);
    b.sys.fail(
        "systemctl --user enable",
        1,
        "Failed to enable unit: Access denied",
    );
    let e = sync::sync(&b.ctx, exe(), &sched(Frequency::Daily, "03:00", &[], 1)).unwrap_err();
    assert!(e.message.contains("Access denied"), "{}", e.message);
}

#[test]
fn a_relative_or_odd_executable_is_refused() {
    let b = bed(Os::Linux, &["systemctl"]);
    let s = sched(Frequency::Daily, "03:00", &[], 1);
    assert!(sync::sync(&b.ctx, Path::new("clearsweep"), &s).is_err());
    assert!(sync::sync(&b.ctx, Path::new("/opt/a\"b/clearsweep"), &s).is_err());
    assert!(b.sys.calls().is_empty());
}

#[test]
fn cron_fallback_install_update_disable_remove() {
    let b = bed(Os::Linux, &["crontab"]);
    let existing = "# mine\n*/5 * * * * /usr/bin/true\n@reboot /home/u/x.sh\n";
    b.sys.set_crontab(Some(existing));
    let mut s = sched(Frequency::Weekly, "03:00", &[1, 3], 1);

    sync::sync(&b.ctx, exe(), &s).unwrap();
    let line = cron_line(&s, EXE).unwrap();
    assert_eq!(b.sys.crontab().unwrap(), format!("{existing}{line}\n"));
    assert_eq!(b.sys.calls_of("crontab")[0], "-l");
    assert!(!unit_dir(&b).exists(), "no systemd files with cron");

    // Update: replaced in place.
    s.time = "05:30".into();
    sync::sync(&b.ctx, exe(), &s).unwrap();
    assert_eq!(
        b.sys.crontab().unwrap(),
        format!("{existing}{}\n", cron_line(&s, EXE).unwrap())
    );

    // Unchanged: crontab is not rewritten.
    b.sys.clear_calls();
    sync::sync(&b.ctx, exe(), &s).unwrap();
    assert_eq!(b.sys.calls_of("crontab"), ["-l"]);

    // Disabled: the line goes, everything else stays.
    s.enabled = false;
    sync::sync(&b.ctx, exe(), &s).unwrap();
    assert_eq!(b.sys.crontab().unwrap(), existing);

    // Enabled again, then removed.
    s.enabled = true;
    sync::sync(&b.ctx, exe(), &s).unwrap();
    assert!(sync::remove(&b.ctx, ID).is_empty());
    assert_eq!(b.sys.crontab().unwrap(), existing);
    // No temp crontab files are left behind.
    let tmpdir = b.ctx.env.data_dir.join("tmp");
    if tmpdir.exists() {
        assert_eq!(std::fs::read_dir(tmpdir).unwrap().count(), 0);
    }
}

#[test]
fn cron_starts_from_an_empty_crontab_and_never_overwrites_an_unreadable_one() {
    let b = bed(Os::Linux, &["crontab"]);
    let s = sched(Frequency::Daily, "03:00", &[], 1);
    b.sys.set_crontab(None); // "no crontab for user"
    sync::sync(&b.ctx, exe(), &s).unwrap();
    assert_eq!(
        b.sys.crontab().unwrap(),
        format!("{}\n", cron_line(&s, EXE).unwrap())
    );

    // `crontab -l` failing for any other reason must abort: rewriting would drop the user's jobs.
    let b = bed(Os::Linux, &["crontab"]);
    b.sys.set_crontab(Some("0 0 * * * precious\n"));
    b.sys.fail("crontab -l", 1, "crontab: permission denied");
    let e = sync::sync(&b.ctx, exe(), &s).unwrap_err();
    assert!(
        e.message.contains("could not read the crontab"),
        "{}",
        e.message
    );
    assert_eq!(b.sys.crontab().unwrap(), "0 0 * * * precious\n");
    // A rejected new crontab is an error too.
    let b = bed(Os::Linux, &["crontab"]);
    b.sys.fail("crontab /", 1, "bad minute");
    assert!(sync::sync(&b.ctx, exe(), &s).is_err());
}

#[test]
fn on_login_is_an_xdg_autostart_entry_with_either_backend() {
    for programs in [&["systemctl", "crontab"][..], &["crontab"][..], &[][..]] {
        let b = bed(Os::Linux, programs);
        let mut s = sched(Frequency::OnLogin, "03:00", &[], 1);
        let path =
            b.fx.env
                .config_dir
                .join(format!("autostart/clearsweep-schedule-{ID}.desktop"));
        sync::sync(&b.ctx, exe(), &s).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            login_desktop_entry(&s, EXE)
        );
        assert!(
            b.sys.calls().is_empty(),
            "no scheduler commands: {:?}",
            b.sys.calls()
        );
        s.enabled = false;
        sync::sync(&b.ctx, exe(), &s).unwrap();
        assert!(!path.exists());
        s.enabled = true;
        sync::sync(&b.ctx, exe(), &s).unwrap();
        assert!(path.exists());
        assert!(sync::remove(&b.ctx, ID).is_empty());
        assert!(!path.exists());
    }
}

#[test]
fn without_a_backend_nothing_is_installed_but_removal_still_cleans() {
    let b = bed(Os::Linux, &[]);
    let s = sched(Frequency::Daily, "03:00", &[], 1);
    sync::sync(&b.ctx, exe(), &s).unwrap();
    assert!(!unit_dir(&b).exists());
    assert!(b.sys.calls().is_empty());
}

#[test]
fn remove_cleans_artifacts_of_every_backend() {
    let b = bed(Os::Linux, &["systemctl", "crontab"]);
    let s = sched(Frequency::Daily, "03:00", &[], 1);
    // Files from an earlier systemd install, a cron line and a login entry all exist.
    sync::sync(&b.ctx, exe(), &s).unwrap();
    let line = cron_line(&s, EXE).unwrap();
    b.sys.set_crontab(Some(&format!("keep\n{line}\n")));
    let l = sched(Frequency::OnLogin, "03:00", &[], 1);
    sync::sync(&b.ctx, exe(), &l).unwrap();
    assert!(sync::remove(&b.ctx, ID).is_empty());
    assert_eq!(b.sys.crontab().unwrap(), "keep\n");
    assert!(!unit_dir(&b).join(format!("clearsweep-{ID}.timer")).exists());
    assert!(!b
        .fx
        .env
        .config_dir
        .join(format!("autostart/clearsweep-schedule-{ID}.desktop"))
        .exists());
    // A hostile id never reaches a path or command.
    b.sys.clear_calls();
    assert!(sync::remove(&b.ctx, "../../x").is_empty());
    assert!(b.sys.calls().is_empty());
}

#[test]
fn windows_task_lifecycle() {
    let b = bed(Os::Windows, &["schtasks", "reg"]);
    let exe = Path::new(r"C:\Program Files\ClearSweep\clearsweep.exe");
    // check_exe wants an absolute path: on this host a drive path is not, so test through
    // the generator plus the sync with an absolute host path.
    let host_exe = Path::new(EXE);
    let mut s = sched(Frequency::Weekly, "03:00", &[1, 3], 1);
    sync::sync(&b.ctx, host_exe, &s).unwrap();
    let want = schtasks_create_args(&s, EXE).join(" ");
    assert_eq!(b.sys.calls(), [format!("schtasks {want}")]);
    let _ = exe;

    b.sys.clear_calls();
    s.enabled = false;
    sync::sync(&b.ctx, host_exe, &s).unwrap();
    let calls = b.sys.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].starts_with("schtasks /create /f /tn ClearSweep\\0123456789abcdef"));
    assert_eq!(
        calls[1],
        format!("schtasks /change /tn ClearSweep\\{ID} /disable")
    );

    b.sys.clear_calls();
    s.enabled = true;
    sync::sync(&b.ctx, host_exe, &s).unwrap();
    assert_eq!(b.sys.calls().len(), 1, "a fresh /create is enabled");

    b.sys.clear_calls();
    b.sys.fail(
        "reg query",
        1,
        "ERROR: The system was unable to find the specified registry key or value.",
    );
    assert!(sync::remove(&b.ctx, ID).is_empty());
    assert_eq!(
        b.sys.calls(),
        [
            format!("schtasks /delete /tn ClearSweep\\{ID} /f"),
            format!(
                r"reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v ClearSweepSchedule-{ID}"
            ),
        ]
    );
    // Deleting a task that does not exist is not an error.
    b.sys.fail(
        "schtasks /delete",
        1,
        "ERROR: The system cannot find the file specified.",
    );
    assert!(sync::remove(&b.ctx, ID).is_empty());
}

#[test]
fn windows_logon_task_falls_back_to_the_run_key() {
    let b = bed(Os::Windows, &["schtasks", "reg"]);
    b.sys
        .fail("schtasks /create", 1, "ERROR: Access is denied.");
    let s = sched(Frequency::OnLogin, "03:00", &[], 1);
    sync::sync(&b.ctx, exe(), &s).unwrap();
    let calls = b.sys.calls();
    assert!(calls[0].starts_with("schtasks /create"));
    assert_eq!(
        calls[1],
        format!(
            r#"reg add HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v ClearSweepSchedule-{ID} /t REG_SZ /d "{EXE}" clean --auto --source scheduled --schedule {ID} /f"#
        )
    );
    // Removal deletes the Run value too when it exists.
    b.sys.clear_calls();
    assert!(sync::remove(&b.ctx, ID).is_empty());
    let calls = b.sys.calls();
    assert!(calls[0].starts_with("schtasks /delete"));
    assert!(calls[1].starts_with("reg query"));
    assert_eq!(
        calls[2],
        format!(
            r"reg delete HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v ClearSweepSchedule-{ID} /f"
        )
    );

    // A refusal for a non-logon schedule is a real error.
    let b = bed(Os::Windows, &["schtasks", "reg"]);
    b.sys
        .fail("schtasks /create", 1, "ERROR: Access is denied.");
    assert!(sync::sync(&b.ctx, exe(), &sched(Frequency::Daily, "03:00", &[], 1)).is_err());
    // Both refused: the message says so.
    let b = bed(Os::Windows, &["schtasks", "reg"]);
    b.sys
        .fail("schtasks /create", 1, "ERROR: Access is denied.");
    b.sys.fail("reg add", 1, "ERROR: nope");
    let e = sync::sync(&b.ctx, exe(), &s).unwrap_err();
    assert!(e.message.contains("fallback"), "{}", e.message);
}

#[test]
fn macos_plist_lifecycle() {
    let b = bed(Os::MacOs, &["launchctl", "id"]);
    let mut s = sched(Frequency::Daily, "03:00", &[], 1);
    let plist_path = b.fx.env.home.join(format!(
        "Library/LaunchAgents/app.clearsweep.schedule.{ID}.plist"
    ));
    sync::sync(&b.ctx, exe(), &s).unwrap();
    assert_eq!(
        std::fs::read_to_string(&plist_path).unwrap(),
        launchd_plist(&s, EXE)
    );
    assert_eq!(
        b.sys.calls(),
        [
            "id -u".to_string(),
            format!("launchctl bootout gui/501/app.clearsweep.schedule.{ID}"),
            format!("launchctl bootstrap gui/501 {}", plist_path.display()),
        ]
    );
    // Disabled: unloaded and the plist removed.
    b.sys.clear_calls();
    s.enabled = false;
    sync::sync(&b.ctx, exe(), &s).unwrap();
    assert!(!plist_path.exists());
    assert_eq!(
        b.sys.calls(),
        [
            "id -u".to_string(),
            format!("launchctl bootout gui/501/app.clearsweep.schedule.{ID}")
        ]
    );
    // Remove after enabling.
    s.enabled = true;
    sync::sync(&b.ctx, exe(), &s).unwrap();
    b.sys.clear_calls();
    assert!(sync::remove(&b.ctx, ID).is_empty());
    assert!(!plist_path.exists());
    assert_eq!(
        b.sys.calls().last().unwrap(),
        &format!("launchctl bootout gui/501/app.clearsweep.schedule.{ID}")
    );
    // A failing bootstrap is an error.
    let b = bed(Os::MacOs, &["launchctl", "id"]);
    b.sys.fail(
        "launchctl bootstrap",
        5,
        "Bootstrap failed: 5: Input/output error",
    );
    let e = sync::sync(&b.ctx, exe(), &s).unwrap_err();
    assert!(e.message.contains("Input/output error"), "{}", e.message);
}

// ---------------------------------------------------------------- API

fn api(b: &Bed, method: &str, p: serde_json::Value) -> Result<serde_json::Value> {
    dispatch(&b.ctx, method, p, &Job::detached())
}

fn linux_bed() -> Bed {
    bed(Os::Linux, &["systemctl", "crontab"])
}

fn add_nightly(b: &Bed) -> serde_json::Value {
    api(
        b,
        "scheduler.add",
        json!({"name": "Nightly", "frequency": "weekly", "time": "03:00", "weekdays": [3, 1]}),
    )
    .unwrap()
}

#[test]
fn add_list_update_toggle_remove_round_trip() {
    let b = linux_bed();
    let v = add_nightly(&b);
    let id = v["id"].as_str().unwrap().to_string();
    assert!(valid_id(&id));
    assert_eq!(v["name"], "Nightly");
    assert_eq!(v["enabled"], true);
    assert_eq!(v["weekdays"], json!([1, 3]));
    assert_eq!(v["dayOfMonth"], 1);
    assert_eq!(v["action"], json!({"kind": "clean", "rules": null}));
    assert!(v["createdAt"].as_str().unwrap().ends_with('Z'));
    assert!(v.get("lastRun").is_none());

    // Persisted, listed, and the OS job exists.
    let list = api(&b, "scheduler.list", json!({})).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    let file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(b.ctx.env.data_dir.join("schedules.json")).unwrap())
            .unwrap();
    assert_eq!(file[0]["id"], id.as_str());
    let timer = unit_dir(&b).join(format!("clearsweep-{id}.timer"));
    let text = std::fs::read_to_string(&timer).unwrap();
    assert!(text.contains("OnCalendar=Mon,Wed *-*-* 03:00:00"), "{text}");
    let service =
        std::fs::read_to_string(unit_dir(&b).join(format!("clearsweep-{id}.service"))).unwrap();
    assert!(
        service.contains(&format!("\"--schedule\" \"{id}\"")),
        "{service}"
    );
    assert!(service.contains("\"scheduled\""));

    // Update re-syncs.
    let u = api(
        &b,
        "scheduler.update",
        json!({"id": id, "time": "04:30", "frequency": "daily"}),
    )
    .unwrap();
    assert_eq!(u["time"], "04:30");
    assert_eq!(u["frequency"], "daily");
    assert_eq!(u["name"], "Nightly");
    assert!(std::fs::read_to_string(&timer)
        .unwrap()
        .contains("OnCalendar=*-*-* 04:30:00"));

    // Disable / enable.
    let d = api(
        &b,
        "scheduler.set_enabled",
        json!({"id": id, "enabled": false}),
    )
    .unwrap();
    assert_eq!(d["enabled"], false);
    assert!(b
        .sys
        .calls()
        .iter()
        .any(|c| c == &format!("systemctl --user disable --now clearsweep-{id}.timer")));
    let e = api(
        &b,
        "scheduler.set_enabled",
        json!({"id": id, "enabled": true}),
    )
    .unwrap();
    assert_eq!(e["enabled"], true);
    assert!(
        b.sys.calls().last().unwrap().contains("restart")
            || b.sys.calls().last().unwrap().contains("enable --now")
    );

    // Remove.
    let r = api(&b, "scheduler.remove", json!({"id": id})).unwrap();
    assert_eq!(r, json!({"removed": true, "warnings": []}));
    assert!(api(&b, "scheduler.list", json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!timer.exists());
    assert_eq!(
        api(&b, "scheduler.remove", json!({"id": id}))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

#[test]
fn api_validation_errors_leave_no_trace() {
    let b = linux_bed();
    add_nightly(&b);
    b.sys.clear_calls();
    let before = std::fs::read(b.ctx.env.data_dir.join("schedules.json")).unwrap();
    let cases = [
        (
            json!({"name": "nightly", "frequency": "daily"}),
            "already a schedule named",
        ),
        (
            json!({"name": "A", "frequency": "daily", "time": "25:00"}),
            "time",
        ),
        (json!({"name": "A", "frequency": "weekly"}), "weekday"),
        (
            json!({"name": "A", "frequency": "monthly", "dayOfMonth": 31}),
            "dayOfMonth",
        ),
        (json!({"name": "A", "frequency": "yearly"}), "yearly"),
        (
            json!({"name": "A", "frequency": "daily", "bogus": 1}),
            "bogus",
        ),
        (
            json!({"name": "A", "frequency": "daily", "action": {"kind": "clean", "rules": ["no.such"]}}),
            "no.such",
        ),
        (json!({"frequency": "daily"}), "name"),
    ];
    for (p, needle) in cases {
        let e = api(&b, "scheduler.add", p.clone()).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams, "{p}");
        assert!(e.message.contains(needle), "{p}: {}", e.message);
    }
    assert_eq!(
        std::fs::read(b.ctx.env.data_dir.join("schedules.json")).unwrap(),
        before
    );
    assert!(b.sys.calls().is_empty(), "{:?}", b.sys.calls());
    // Updates: unknown id, read-only fields, bad values.
    assert_eq!(
        api(&b, "scheduler.update", json!({"id": ID}))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let id = api(&b, "scheduler.list", json!({})).unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(api(&b, "scheduler.update", json!({"id": id, "createdAt": "x"})).is_err());
    assert!(api(&b, "scheduler.update", json!({"id": id, "lastRun": "x"})).is_err());
    assert!(api(&b, "scheduler.update", json!({"id": id, "time": "9"})).is_err());
    assert_eq!(
        std::fs::read(b.ctx.env.data_dir.join("schedules.json")).unwrap(),
        before
    );
}

#[test]
fn a_failing_os_sync_saves_nothing_and_cleans_up() {
    let b = linux_bed();
    b.sys.fail("systemctl --user enable", 1, "boom");
    let e = api(
        &b,
        "scheduler.add",
        json!({"name": "A", "frequency": "daily"}),
    )
    .unwrap_err();
    assert!(e.message.contains("boom"), "{}", e.message);
    assert!(api(&b, "scheduler.list", json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    let leftovers = std::fs::read_dir(unit_dir(&b))
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(leftovers, 0, "half-installed unit files are removed");
}

#[test]
fn update_failure_restores_the_previous_os_state() {
    let b = bed(Os::Linux, &["crontab"]);
    let v = add_nightly(&b);
    let id = v["id"].as_str().unwrap().to_string();
    let before = b.sys.crontab().unwrap();
    b.sys.fail("crontab /", 1, "rejected");
    let e = api(&b, "scheduler.update", json!({"id": id, "time": "05:00"})).unwrap_err();
    assert!(e.message.contains("rejected"), "{}", e.message);
    assert_eq!(b.sys.crontab().unwrap(), before);
    assert_eq!(
        api(&b, "scheduler.list", json!({})).unwrap()[0]["time"],
        "03:00"
    );
}

#[test]
fn switching_to_and_from_on_login_moves_the_artifact() {
    let b = linux_bed();
    let v = add_nightly(&b);
    let id = v["id"].as_str().unwrap().to_string();
    let xdg =
        b.fx.env
            .config_dir
            .join(format!("autostart/clearsweep-schedule-{id}.desktop"));
    let timer = unit_dir(&b).join(format!("clearsweep-{id}.timer"));
    assert!(timer.exists() && !xdg.exists());
    api(
        &b,
        "scheduler.update",
        json!({"id": id, "frequency": "on_login"}),
    )
    .unwrap();
    assert!(xdg.exists() && !timer.exists());
    api(
        &b,
        "scheduler.update",
        json!({"id": id, "frequency": "daily"}),
    )
    .unwrap();
    assert!(!xdg.exists() && timer.exists());
}

#[test]
fn backend_method_reports_the_mechanism() {
    let b = bed(Os::Linux, &["crontab"]);
    let v = api(&b, "scheduler.backend", json!({})).unwrap();
    assert_eq!(v["kind"], "cron");
    assert_eq!(v["available"], true);
    assert!(v["detail"].as_str().unwrap().contains("cron"));
}

#[test]
fn schedules_are_stored_even_when_no_backend_exists() {
    let b = bed(Os::Linux, &[]);
    let v = add_nightly(&b);
    assert_eq!(v["enabled"], true);
    assert_eq!(
        api(&b, "scheduler.list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(b.sys.calls().is_empty());
}

#[test]
fn a_damaged_store_is_reported_not_overwritten() {
    let b = linux_bed();
    std::fs::create_dir_all(&b.ctx.env.data_dir).unwrap();
    let path = b.ctx.env.data_dir.join("schedules.json");
    std::fs::write(&path, "{not json").unwrap();
    let e = api(&b, "scheduler.list", json!({})).unwrap_err();
    assert!(e.message.contains("damaged"), "{}", e.message);
    assert!(api(
        &b,
        "scheduler.add",
        json!({"name": "A", "frequency": "daily"})
    )
    .is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
    // An entry with an unsafe id is refused too.
    std::fs::write(
        &path,
        r#"[{"id":"../evil","name":"x","frequency":"daily","createdAt":"2024-01-01T00:00:00Z"}]"#,
    )
    .unwrap();
    assert!(api(&b, "scheduler.list", json!({}))
        .unwrap_err()
        .message
        .contains("invalid id"));
    // Older / hand-written entries with missing optional fields load with defaults.
    std::fs::write(
        &path,
        format!(
            r#"[{{"id":"{ID}","name":"x","frequency":"daily","createdAt":"2024-01-01T00:00:00Z"}}]"#
        ),
    )
    .unwrap();
    let v = api(&b, "scheduler.list", json!({})).unwrap();
    assert_eq!(v[0]["time"], "03:00");
    assert_eq!(v[0]["enabled"], true);
    // An empty file is an empty list.
    std::fs::write(&path, "").unwrap();
    assert!(api(&b, "scheduler.list", json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn the_number_of_schedules_is_capped() {
    let b = bed(Os::Linux, &[]);
    for i in 0..MAX_SCHEDULES {
        api(
            &b,
            "scheduler.add",
            json!({"name": format!("s{i}"), "frequency": "daily"}),
        )
        .unwrap();
    }
    assert!(api(
        &b,
        "scheduler.add",
        json!({"name": "one too many", "frequency": "daily"})
    )
    .is_err());
}

// ---------------------------------------------------------------- running

fn chrome_bed() -> (Bed, std::path::PathBuf, std::path::PathBuf) {
    let b = linux_bed();
    let p = b.fx.populate_chromium(Chromium::Chrome, "Default");
    let cache = p.cache.join("Cache/Cache_Data/data_0");
    let gpu = p.cache.join("GPUCache/data_0");
    (b, cache, gpu)
}

#[test]
fn run_now_cleans_records_last_run_and_uses_the_scheduled_source() {
    let (b, cache, gpu) = chrome_bed();
    let v = api(&b, "scheduler.add", json!({"name": "Chrome", "frequency": "daily", "action": {"kind": "clean", "rules": ["chrome.cache"]}})).unwrap();
    let id = v["id"].as_str().unwrap();
    let out = api(&b, "scheduler.run_now", json!({"id": id})).unwrap();
    assert!(!cache.exists() && !gpu.exists());
    assert_eq!(out["report"]["totalFiles"], 5);
    let s = &out["schedule"];
    assert!(s["lastRun"].as_str().unwrap().ends_with('Z'));
    assert_eq!(s["lastResult"]["ok"], true);
    assert_eq!(s["lastResult"]["totalFiles"], 5);
    assert_eq!(s["lastResult"]["totalBytes"], 100 * 1024 + 4096);
    // Persisted.
    let list = api(&b, "scheduler.list", json!({})).unwrap();
    assert_eq!(list[0]["lastResult"]["totalFiles"], 5);
    // History says "scheduled".
    let h = history::list(&b.ctx, Some(1));
    assert_eq!(h[0].source, Source::Scheduled);
    assert_eq!(h[0].rule_ids, ["chrome.cache"]);
}

#[test]
fn run_now_only_runs_the_schedules_own_rules_and_works_when_disabled() {
    let (b, cache, _) = chrome_bed();
    let history_db =
        b.fx.chromium_profile(Chromium::Chrome, "Default")
            .data
            .join("History");
    let v = api(&b, "scheduler.add", json!({"name": "Chrome", "frequency": "daily", "enabled": false, "action": {"kind": "clean", "rules": ["chrome.cache"]}})).unwrap();
    api(&b, "scheduler.run_now", json!({"id": v["id"]})).unwrap();
    assert!(!cache.exists());
    assert!(history_db.exists());
    let rows: i64 = rusqlite::Connection::open(&history_db)
        .unwrap()
        .query_row("SELECT count(*) FROM urls", [], |r| r.get(0))
        .unwrap();
    assert!(rows > 0, "history rows untouched");
}

#[test]
fn run_now_without_rules_uses_the_enabled_selection() {
    let (b, cache, _) = chrome_bed();
    crate::features::settings::update(&b.ctx, |s| {
        s.selected_rules = Some(vec!["chrome.cache".into()])
    })
    .unwrap();
    let v = api(
        &b,
        "scheduler.add",
        json!({"name": "Sel", "frequency": "daily"}),
    )
    .unwrap();
    let out = api(&b, "scheduler.run_now", json!({"id": v["id"]})).unwrap();
    assert!(!cache.exists());
    assert_eq!(out["schedule"]["lastResult"]["ok"], true);
}

#[test]
fn a_failing_run_is_recorded() {
    let (b, _, _) = chrome_bed();
    // Hand-edit a schedule that names a rule that does not exist (a rule removed by an update).
    let path = b.ctx.env.data_dir.join("schedules.json");
    std::fs::create_dir_all(&b.ctx.env.data_dir).unwrap();
    std::fs::write(
        &path,
        format!(r#"[{{"id":"{ID}","name":"x","frequency":"daily","createdAt":"2024-01-01T00:00:00Z","action":{{"kind":"clean","rules":["gone.rule"]}}}}]"#),
    )
    .unwrap();
    let e = api(&b, "scheduler.run_now", json!({"id": ID})).unwrap_err();
    assert!(e.message.contains("gone.rule"), "{}", e.message);
    let list = api(&b, "scheduler.list", json!({})).unwrap();
    assert_eq!(list[0]["lastResult"]["ok"], false);
    assert!(list[0]["lastResult"]["message"]
        .as_str()
        .unwrap()
        .contains("gone.rule"));
    assert!(list[0]["lastRun"].is_string());
}

#[test]
fn os_launched_runs_refuse_a_disabled_schedule() {
    let (b, cache, _) = chrome_bed();
    let v = api(&b, "scheduler.add", json!({"name": "Chrome", "frequency": "daily", "enabled": false, "action": {"kind": "clean", "rules": ["chrome.cache"]}})).unwrap();
    let id = v["id"].as_str().unwrap();
    let e = run_schedule(&b.ctx, id, &Job::detached(), true)
        .err()
        .unwrap();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    assert!(cache.exists());
    assert_eq!(
        run_schedule(&b.ctx, ID, &Job::detached(), true)
            .err()
            .unwrap()
            .code,
        ErrorCode::NotFound
    );
}

// ---------------------------------------------------------------- catch-up

struct Clk {
    ms: AtomicU64,
    local: LocalTime,
}

impl Clock for Clk {
    fn now_ms(&self) -> u64 {
        self.ms.load(Ordering::SeqCst)
    }
    fn local(&self) -> Option<LocalTime> {
        Some(self.local)
    }
}

/// 2024-01-03 (Wednesday) 09:00 local, which the tests treat as 09:00 UTC too.
fn clock_0900() -> Clk {
    Clk {
        ms: AtomicU64::new(1_704_272_400_000),
        local: lt(2024, 1, 3, 9, 0),
    }
}

fn write_schedule(b: &Bed, f: impl FnOnce(&mut Schedule)) {
    let mut s = sched(Frequency::Daily, "03:00", &[], 1);
    s.action.rules = Some(vec!["chrome.cache".into()]);
    f(&mut s);
    std::fs::create_dir_all(&b.ctx.env.data_dir).unwrap();
    std::fs::write(
        b.ctx.env.data_dir.join("schedules.json"),
        serde_json::to_vec(&[s]).unwrap(),
    )
    .unwrap();
}

fn catch(b: &Bed, clk: &Clk) -> Vec<(String, std::result::Result<u64, String>)> {
    catch_up(&b.ctx, clk, &Job::detached())
}

#[test]
fn catch_up_runs_a_missed_daily_clean_once() {
    let (b, cache, _) = chrome_bed();
    write_schedule(&b, |s| s.created_at = "2024-01-01T00:00:00Z".into());
    let clk = clock_0900();
    let ran = catch(&b, &clk);
    assert_eq!(ran.len(), 1, "{ran:?}");
    assert_eq!(ran[0].0, ID);
    assert!(ran[0].1.as_ref().unwrap() > &0);
    assert!(!cache.exists());
    let list = load(&b.ctx).unwrap();
    assert!(list[0].last_run.is_some());
    // The run is recorded, so it is not repeated.
    assert!(catch(&b, &clk).is_empty());
}

type Tweak = Box<dyn FnOnce(&mut Schedule)>;

#[test]
fn catch_up_skips_what_does_not_need_it() {
    let (b, cache, _) = chrome_bed();
    let clk = clock_0900();
    let cases: Vec<(&str, Tweak)> = vec![
        (
            "created after the due time",
            Box::new(|s| s.created_at = "2024-01-03T04:00:00Z".into()),
        ),
        (
            "ran after the due time",
            Box::new(|s| s.last_run = Some("2024-01-03T03:00:05Z".into())),
        ),
        ("disabled", Box::new(|s| s.enabled = false)),
        ("on login", Box::new(|s| s.frequency = Frequency::OnLogin)),
        (
            "due within the grace period",
            Box::new(|s| {
                s.time = "08:55".into();
            }),
        ),
        (
            "weekly, and the last Mon/Wed 03:00 was run",
            Box::new(|s| {
                s.frequency = Frequency::Weekly;
                s.weekdays = vec![1, 3];
                s.last_run = Some("2024-01-03T03:00:10Z".into());
            }),
        ),
        (
            "monthly, ran on the 15th",
            Box::new(|s| {
                s.frequency = Frequency::Monthly;
                s.day_of_month = 15;
                s.last_run = Some("2023-12-15T03:00:10Z".into());
            }),
        ),
    ];
    for (why, f) in cases {
        write_schedule(&b, f);
        assert!(catch(&b, &clk).is_empty(), "{why}");
        assert!(cache.exists(), "{why}");
    }
}

#[test]
fn catch_up_covers_weekly_monthly_and_hourly() {
    for (label, f) in [
        (
            "weekly",
            Box::new(|s: &mut Schedule| {
                s.frequency = Frequency::Weekly;
                s.weekdays = vec![1];
                s.last_run = Some("2023-12-25T03:00:00Z".into());
            }) as Tweak,
        ),
        (
            "monthly",
            Box::new(|s: &mut Schedule| {
                s.frequency = Frequency::Monthly;
                s.day_of_month = 1;
                s.last_run = Some("2023-12-01T03:00:00Z".into());
            }),
        ),
        (
            "hourly",
            Box::new(|s: &mut Schedule| {
                s.frequency = Frequency::Hourly;
                s.time = "00:20".into();
                s.last_run = Some("2024-01-03T07:20:00Z".into());
            }),
        ),
    ] {
        let (b, cache, _) = chrome_bed();
        write_schedule(&b, f);
        let ran = catch(&b, &clock_0900());
        assert_eq!(ran.len(), 1, "{label}: {ran:?}");
        assert!(!cache.exists(), "{label}");
    }
}

#[test]
fn catch_up_needs_the_local_time_and_a_readable_store() {
    struct NoLocal;
    impl Clock for NoLocal {
        fn now_ms(&self) -> u64 {
            1_704_272_400_000
        }
        fn local(&self) -> Option<LocalTime> {
            None
        }
    }
    let (b, cache, _) = chrome_bed();
    write_schedule(&b, |_| {});
    assert!(catch_up(&b.ctx, &NoLocal, &Job::detached()).is_empty());
    assert!(cache.exists());
    std::fs::write(b.ctx.env.data_dir.join("schedules.json"), "junk").unwrap();
    assert!(catch(&b, &clock_0900()).is_empty());
}

#[test]
fn catch_up_records_failures_without_crashing() {
    let (b, _, _) = chrome_bed();
    write_schedule(&b, |s| s.action.rules = Some(vec!["gone.rule".into()]));
    let ran = catch(&b, &clock_0900());
    assert_eq!(ran.len(), 1);
    assert!(ran[0].1.as_ref().unwrap_err().contains("gone.rule"));
    // Recorded as a run, so it is not retried every minute.
    assert!(catch(&b, &clock_0900()).is_empty());
}
