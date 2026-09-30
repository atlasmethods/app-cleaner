//! Health check tests: the score formula (table tests), every category built from fixtures
//! and mocks, independence of the categories, "fix does only what is listed", server-side
//! re-validation of ids, cancellation between parts and the stored last result.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::model::*;
use super::score::{self, Inputs, PrivacyIn, SecurityIn, SpaceIn, SpeedIn};
use super::*;
use crate::api::dispatch;
use crate::ctx::Os;
use crate::elevate::with_elevation;
use crate::error::ErrorCode;
use crate::features::cleaner;
use crate::features::settings;
use crate::features::startup::Impact;
use crate::job::{CancelToken, Job, ProgressEvent};
use crate::procs::{FakeProcesses, ProcDetail};
use crate::runner::{CmdOutput, MockRunner};
use crate::testutil::{query_i64, Chromium, Fixture};

const MB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MB;

// ================================================================ the score formula

#[test]
fn score_table() {
    struct Case {
        name: &'static str,
        inputs: Inputs,
        want: Option<u8>,
    }
    let all = |space, privacy, speed, security| Inputs {
        space: Some(space),
        privacy: Some(privacy),
        speed: Some(speed),
        security: Some(security),
    };
    let none_space = SpaceIn::default();
    let none_privacy = PrivacyIn::default();
    let none_speed = SpeedIn::default();
    let none_security = SecurityIn::default();
    let cases = vec![
        Case {
            name: "nothing found",
            inputs: all(none_space, none_privacy, none_speed, none_security),
            want: Some(100),
        },
        Case {
            name: "1 GiB of junk costs 10",
            inputs: all(
                SpaceIn { junk_bytes: GIB },
                none_privacy,
                none_speed,
                none_security,
            ),
            want: Some(90),
        },
        Case {
            name: "2.5 GiB of junk reaches the space cap (25)",
            inputs: all(
                SpaceIn {
                    junk_bytes: 5 * GIB / 2,
                },
                none_privacy,
                none_speed,
                none_security,
            ),
            want: Some(75),
        },
        Case {
            name: "space is capped at 25 however much junk there is",
            inputs: all(
                SpaceIn {
                    junk_bytes: 500 * GIB,
                },
                none_privacy,
                none_speed,
                none_security,
            ),
            want: Some(75),
        },
        Case {
            name: "50 trackers cost 10",
            inputs: all(
                none_space,
                PrivacyIn {
                    trackers: 50,
                    history_rows: 0,
                },
                none_speed,
                none_security,
            ),
            want: Some(90),
        },
        Case {
            name: "history rows cost 0.02 each: 500 rows = 10",
            inputs: all(
                none_space,
                PrivacyIn {
                    trackers: 0,
                    history_rows: 500,
                },
                none_speed,
                none_security,
            ),
            want: Some(90),
        },
        Case {
            name: "privacy is capped at 20",
            inputs: all(
                none_space,
                PrivacyIn {
                    trackers: 100_000,
                    history_rows: 100_000,
                },
                none_speed,
                none_security,
            ),
            want: Some(80),
        },
        Case {
            name: "1 high + 2 medium items cost 6 + 4",
            inputs: all(
                none_space,
                none_privacy,
                SpeedIn {
                    high_impact: 1,
                    medium_impact: 2,
                    background_apps: 0,
                },
                none_security,
            ),
            want: Some(90),
        },
        Case {
            name: "background apps cost 1 each",
            inputs: all(
                none_space,
                none_privacy,
                SpeedIn {
                    high_impact: 0,
                    medium_impact: 0,
                    background_apps: 3,
                },
                none_security,
            ),
            want: Some(97),
        },
        Case {
            name: "speed is capped at 25",
            inputs: all(
                none_space,
                none_privacy,
                SpeedIn {
                    high_impact: 50,
                    medium_impact: 50,
                    background_apps: 50,
                },
                none_security,
            ),
            want: Some(75),
        },
        Case {
            name: "3 regular updates cost 3",
            inputs: all(
                none_space,
                none_privacy,
                none_speed,
                SecurityIn {
                    updates: 3,
                    security: 0,
                },
            ),
            want: Some(97),
        },
        Case {
            name: "security updates weigh 5 each: 2 security + 1 regular = 11",
            inputs: all(
                none_space,
                none_privacy,
                none_speed,
                SecurityIn {
                    updates: 3,
                    security: 2,
                },
            ),
            want: Some(89),
        },
        Case {
            name: "security is capped at 30",
            inputs: all(
                none_space,
                none_privacy,
                none_speed,
                SecurityIn {
                    updates: 400,
                    security: 100,
                },
            ),
            want: Some(70),
        },
        Case {
            name: "everything at its cap scores 0",
            inputs: all(
                SpaceIn {
                    junk_bytes: 100 * GIB,
                },
                PrivacyIn {
                    trackers: 10_000,
                    history_rows: 0,
                },
                SpeedIn {
                    high_impact: 10,
                    medium_impact: 0,
                    background_apps: 0,
                },
                SecurityIn {
                    updates: 100,
                    security: 100,
                },
            ),
            want: Some(0),
        },
        Case {
            name: "unavailable security: the other weights are renormalized (10 of 70 -> 14.3)",
            inputs: Inputs {
                space: Some(SpaceIn { junk_bytes: GIB }),
                privacy: Some(none_privacy),
                speed: Some(none_speed),
                security: None,
            },
            want: Some(86),
        },
        Case {
            name: "unavailable security with every other category at its cap is still 0",
            inputs: Inputs {
                space: Some(SpaceIn {
                    junk_bytes: 100 * GIB,
                }),
                privacy: Some(PrivacyIn {
                    trackers: 10_000,
                    history_rows: 0,
                }),
                speed: Some(SpeedIn {
                    high_impact: 10,
                    medium_impact: 0,
                    background_apps: 0,
                }),
                security: None,
            },
            want: Some(0),
        },
        Case {
            name: "only one category available: its own cap is the whole scale",
            inputs: Inputs {
                security: Some(SecurityIn {
                    updates: 15,
                    security: 0,
                }),
                ..Inputs::default()
            },
            want: Some(50),
        },
        Case {
            name: "no category available: no score",
            inputs: Inputs::default(),
            want: None,
        },
    ];
    for c in cases {
        assert_eq!(score::overall(&c.inputs), c.want, "{}", c.name);
    }
}

#[test]
fn score_never_increases_when_things_get_worse() {
    let mut prev = 101i32;
    for gb10 in 0..=40u64 {
        let s = score::overall(&Inputs {
            space: Some(SpaceIn {
                junk_bytes: gb10 * GIB / 10,
            }),
            privacy: Some(PrivacyIn::default()),
            speed: Some(SpeedIn::default()),
            security: Some(SecurityIn::default()),
        })
        .unwrap() as i32;
        assert!(s <= prev, "junk {gb10}/10 GiB: {s} > {prev}");
        prev = s;
    }
    let mut prev = 101i32;
    for updates in 0..=40u64 {
        let s = score::overall(&Inputs {
            security: Some(SecurityIn {
                updates,
                security: updates / 2,
            }),
            ..Inputs::default()
        })
        .unwrap() as i32;
        assert!(s <= prev);
        prev = s;
    }
}

#[test]
fn caps_add_up_to_100() {
    assert_eq!(
        score::CAP_SPACE + score::CAP_PRIVACY + score::CAP_SPEED + score::CAP_SECURITY,
        100.0
    );
}

fn cat(id: CategoryId, status: Status, metrics: &[(&str, u64)]) -> CategoryReport {
    let mut c = CategoryReport::unavailable(id, "x");
    c.status = status;
    for (k, v) in metrics {
        c.metrics.insert((*k).to_string(), *v);
    }
    c
}

#[test]
fn inputs_are_read_from_metrics_and_unavailable_is_left_out() {
    let cats = vec![
        cat(CategoryId::Privacy, Status::Warning, &[("trackers", 50)]),
        cat(CategoryId::Space, Status::Good, &[("bytes", GIB)]),
        cat(CategoryId::Speed, Status::Unavailable, &[("highImpact", 9)]),
        cat(
            CategoryId::Security,
            Status::Problem,
            &[("updates", 3), ("securityUpdates", 2)],
        ),
    ];
    let i = score::inputs_of(&cats);
    assert_eq!(
        i.privacy,
        Some(PrivacyIn {
            trackers: 50,
            history_rows: 0
        })
    );
    assert_eq!(i.space, Some(SpaceIn { junk_bytes: GIB }));
    assert_eq!(i.speed, None, "unavailable metrics are ignored");
    assert_eq!(
        i.security,
        Some(SecurityIn {
            updates: 3,
            security: 2
        })
    );
    // 10 (space) + 10 (privacy) + 11 (security) of caps 25 + 20 + 30 = 75 -> scale 100/75
    let want = (100.0f64 - 31.0 * 100.0 / 75.0).round() as u8;
    assert_eq!(score::score_of(&cats), Some(want));
}

// ================================================================ test bed

struct Bed {
    _tmp: tempfile::TempDir,
    fx: Fixture,
    ctx: Ctx,
    mock: MockRunner,
    procs: Arc<FakeProcesses>,
}

fn proc(pid: u32, name: &str, exe: &str, mb: u64) -> ProcDetail {
    ProcDetail {
        pid,
        name: name.into(),
        exe: (!exe.is_empty()).then(|| exe.into()),
        memory_bytes: mb * MB,
        cpu_percent: 1.0,
        is_mine: true,
    }
}

fn bed_with(procs: Vec<ProcDetail>, terminate: bool) -> Bed {
    let tmp = tempfile::tempdir().unwrap();
    let mut fx = Fixture::new(tmp.path());
    fx.env.os = Os::Linux;
    let mock = MockRunner::new();
    let procs = Arc::new(FakeProcesses::with_details(procs, terminate));
    let ctx = Ctx::new(fx.env.clone(), Arc::new(mock.clone())).with_procs(procs.clone());
    Bed {
        _tmp: tmp,
        fx,
        ctx,
        mock,
        procs,
    }
}

fn bed() -> Bed {
    bed_with(Vec::new(), true)
}

impl Bed {
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        dispatch(&self.ctx, method, params, &Job::detached())
    }
    fn analyze(&self) -> HealthReport {
        run_analyze(&self.ctx, &Job::detached()).unwrap()
    }
    fn fix(&self, params: Value) -> FixReport {
        serde_json::from_value(self.call("health.fix", params).unwrap()).unwrap()
    }
    fn set_keep(&self, domains: &[&str]) {
        settings::update(&self.ctx, |s| {
            s.cookie_keep = domains.iter().map(|d| d.to_string()).collect();
        })
        .unwrap();
    }
    fn autostart(&self, file: &str, text: &str) -> PathBuf {
        let p = crate::features::startup::linux::user_autostart_dir(&self.ctx).join(file);
        write(&p, text);
        p
    }
    fn system_autostart(&self, file: &str, text: &str) -> PathBuf {
        let p = crate::features::startup::linux::system_autostart_dir(&self.ctx).join(file);
        write(&p, text);
        p
    }
    fn chrome(&self) -> crate::testutil::ChromiumProfile {
        self.fx.populate_chromium(Chromium::Chrome, "Default")
    }
    /// Programs that changed something on the machine (anything but read-only listings).
    fn mutating_calls(&self) -> Vec<String> {
        self.mock
            .calls()
            .into_iter()
            .filter(|(p, a)| {
                let line = format!("{p} {}", a.join(" "));
                let read_only = [
                    "flatpak remote-ls",
                    "snap refresh --list",
                    "dnf check-update",
                    "crontab -l",
                    "systemctl list-unit-files",
                    "systemctl --user list-unit-files",
                ]
                .iter()
                .any(|r| line.starts_with(r));
                match p.as_str() {
                    "apt-get" | "pkexec" | "osascript" | "taskkill" | "dnf" | "snap"
                    | "flatpak" | "crontab" => !read_only,
                    "systemctl" => a.iter().any(|x| x == "enable" || x == "disable"),
                    _ => false,
                }
            })
            .map(|(p, a)| format!("{p} {}", a.join(" ")))
            .collect()
    }
}

fn write(p: &Path, text: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn cat_of(r: &HealthReport, id: CategoryId) -> &CategoryReport {
    r.categories.iter().find(|c| c.id == id).unwrap()
}

fn part(r: &FixReport, p: Part) -> &PartResult {
    r.parts
        .iter()
        .find(|x| x.part == p)
        .unwrap_or_else(|| panic!("no part {p:?} in {:?}", r.parts))
}

const APT: &str = "\
Listing...
firefox/noble-updates,noble-security 126.0 amd64 [upgradable from: 125.0]
vim/noble-updates 2:9.1 amd64 [upgradable from: 2:9.0]
htop/noble 3.3.0 amd64 [upgradable from: 3.2.0]
";

fn script_apt(m: &MockRunner) {
    m.on("apt", &["list", "--upgradable"], CmdOutput::ok(APT));
}

fn desktop(name: &str, exec: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n")
}

/// Slack (300 MB, high), Spotify (60 MB, medium), a small tray tool (5 MB, low), a security
/// tool, a critical session agent and a disabled item that is still running.
fn speed_bed() -> Bed {
    let b = bed_with(
        vec![
            proc(100, "slack", "/opt/Slack/slack", 300),
            proc(101, "spotify", "/opt/spotify/spotify", 60),
            proc(102, "tiny", "/opt/tiny/tiny", 5),
            proc(103, "mbam", "/opt/malwarebytes/mbam", 300),
            proc(
                104,
                "polkit-gnome-au",
                "/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1",
                20,
            ),
            proc(105, "olddisabled", "/opt/old/olddisabled", 400),
            proc(106, "firefox", "/usr/lib/firefox/firefox", 500),
        ],
        true,
    );
    b.autostart("slack.desktop", &desktop("Slack", "/opt/Slack/slack -u %U"));
    b.autostart(
        "spotify.desktop",
        &desktop("Spotify", "/opt/spotify/spotify"),
    );
    b.autostart("tiny.desktop", &desktop("Tiny", "/opt/tiny/tiny"));
    b.autostart(
        "malwarebytes.desktop",
        &desktop("Malwarebytes", "/opt/malwarebytes/mbam"),
    );
    b.autostart(
        "old.desktop",
        &format!("{}Hidden=true\n", desktop("Old", "/opt/old/olddisabled")),
    );
    b.system_autostart(
        "polkit-gnome-authentication-agent-1.desktop",
        &desktop(
            "PolicyKit",
            "/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1",
        ),
    );
    // Firefox is a running installed app but has no startup item: not a background app.
    write(
        &b.ctx
            .env
            .sys_path("/usr/share/applications/firefox.desktop"),
        &desktop("Firefox", "/usr/lib/firefox/firefox %u"),
    );
    b
}

// ================================================================ analyze: categories

#[test]
fn analysis_has_four_categories_in_order_with_titles() {
    let b = bed();
    script_apt(&b.mock);
    let r = b.analyze();
    let ids: Vec<CategoryId> = r.categories.iter().map(|c| c.id).collect();
    assert_eq!(
        ids,
        [
            CategoryId::Privacy,
            CategoryId::Space,
            CategoryId::Speed,
            CategoryId::Security
        ]
    );
    let titles: Vec<&str> = r.categories.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, ["Privacy", "Space", "Speed", "Security"]);
    assert!(r.scanned_at.ends_with('Z') || r.scanned_at.contains('+'));
}

#[test]
fn wire_shape_is_camel_case_with_kind_tags() {
    let b = bed();
    b.chrome();
    script_apt(&b.mock);
    let v = b.call("health.analyze", json!({})).unwrap();
    assert!(v["score"].is_u64());
    assert!(v["scannedAt"].is_string());
    let privacy = &v["categories"][0];
    assert_eq!(privacy["id"], "privacy");
    assert_eq!(privacy["status"], "warning");
    assert_eq!(privacy["fixable"], true);
    assert_eq!(privacy["findings"][0]["kind"], "trackers");
    assert!(privacy["findings"][0]["count"].as_u64().unwrap() > 0);
    assert_eq!(privacy["findings"][0]["browsers"], json!(["Google Chrome"]));
    let sec = &v["categories"][3];
    assert_eq!(sec["findings"][0]["kind"], "updates");
    assert_eq!(sec["findings"][0]["items"][0]["newVersion"], "126.0");
    assert_eq!(sec["findings"][0]["items"][0]["currentVersion"], "125.0");
}

#[test]
fn privacy_counts_only_cookies_off_the_keep_list() {
    let b = bed();
    b.chrome();
    b.set_keep(&["google.com", "github.com"]);
    let r = b.analyze();
    let p = cat_of(&r, CategoryId::Privacy);
    // notgoogle.com (1) + tracker.example (2); google.com (3) and github.com (1) are kept.
    assert_eq!(p.metric("trackers"), 3);
    assert!(
        p.metric("historyRows") > 0,
        "history / download rows are counted"
    );
    assert!(p.fixable);
    assert_eq!(p.status, Status::Warning);
    assert!(
        matches!(&p.findings[0], Finding::Trackers { count: 3, browsers } if browsers == &["Google Chrome"])
    );
    assert!(p
        .findings
        .iter()
        .any(|f| matches!(f, Finding::History { .. })));
    assert!(
        p.summary.contains("3 tracking cookies in 1 browser"),
        "{}",
        p.summary
    );

    // With nothing kept every cookie is a tracker: 3 + 1 + 3.
    b.set_keep(&[]);
    let p = cat_of(&b.analyze(), CategoryId::Privacy).clone();
    assert_eq!(p.metric("trackers"), 7);
}

#[test]
fn privacy_spans_browsers_and_names_them() {
    let b = bed();
    b.chrome();
    b.fx.populate_firefox("abcd.default-release");
    b.set_keep(&["google.com", "github.com", "mozilla.org"]);
    let p = cat_of(&b.analyze(), CategoryId::Privacy).clone();
    // Chrome: 3, Firefox: ads.example (2) + notmozilla.org (1).
    assert_eq!(p.metric("trackers"), 6);
    let Finding::Trackers { browsers, .. } = &p.findings[0] else {
        panic!()
    };
    assert_eq!(
        browsers,
        &["Google Chrome".to_string(), "Mozilla Firefox".to_string()]
    );
}

#[test]
fn privacy_is_good_when_there_is_nothing() {
    let b = bed();
    let p = cat_of(&b.analyze(), CategoryId::Privacy).clone();
    assert_eq!(p.status, Status::Good);
    assert!(!p.fixable);
    assert!(p.findings.is_empty());
    assert_eq!(p.metric("trackers"), 0);
}

#[test]
fn privacy_follows_the_enabled_rule_selection() {
    let b = bed();
    b.chrome();
    // The user turned every cookie / history / download rule off on the Clean tab.
    settings::update(&b.ctx, |s| {
        s.selected_rules = Some(vec!["chrome.cache".into()])
    })
    .unwrap();
    let p = cat_of(&b.analyze(), CategoryId::Privacy).clone();
    assert_eq!(p.status, Status::Good);
    assert_eq!(p.metric("trackers"), 0);
}

#[test]
fn rule_selection_splits_into_privacy_and_space_without_overlap() {
    let b = bed();
    let s = settings::load(&b.ctx);
    let (privacy, space) = analyze::enabled_rule_ids(&b.ctx, &s).unwrap();
    assert!(privacy.contains(&"chrome.cookies".to_string()));
    assert!(privacy.contains(&"firefox.history".to_string()));
    assert!(privacy.iter().all(|i| {
        i.ends_with(".cookies") || i.ends_with(".history") || i.ends_with(".downloads")
    }));
    assert!(space.contains(&"chrome.cache".to_string()));
    assert!(space.iter().all(|i| !privacy.contains(i)));
    let mut both: Vec<String> = privacy.iter().chain(space.iter()).cloned().collect();
    let mut enabled: Vec<String> = cleaner::select_rules(&b.ctx, &s, None)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    both.sort();
    enabled.sort();
    assert_eq!(
        both, enabled,
        "together they are exactly the Clean tab's selection"
    );
}

#[test]
fn space_matches_the_cleaners_analysis_of_the_same_selection() {
    let b = bed();
    b.chrome();
    b.fx.populate_linux_system();
    let r = b.analyze();
    let sp = cat_of(&r, CategoryId::Space);
    let s = settings::load(&b.ctx);
    let (_, ids) = analyze::enabled_rule_ids(&b.ctx, &s).unwrap();
    let rep = cleaner::analyze(&b.ctx, Some(ids), &Job::detached()).unwrap();
    assert!(rep.total_bytes > 0);
    assert_eq!(sp.metric("bytes"), rep.total_bytes);
    assert_eq!(sp.metric("files"), rep.total_files);
    assert!(sp.fixable);
    assert!(sp.summary.contains("of junk in"), "{}", sp.summary);
    // The top groups by size, largest first, at most five.
    let groups: Vec<(&str, u64)> = sp
        .findings
        .iter()
        .map(|f| match f {
            Finding::Junk { group, bytes, .. } => (group.as_str(), *bytes),
            other => panic!("unexpected finding {other:?}"),
        })
        .collect();
    assert!(!groups.is_empty() && groups.len() <= 5);
    assert!(groups.windows(2).all(|w| w[0].1 >= w[1].1), "{groups:?}");
    // Cookies / history are the privacy category's business, not counted as junk.
    let ck = cat_of(&r, CategoryId::Privacy);
    assert!(ck.metric("trackers") > 0);
}

#[test]
fn space_status_is_problem_from_one_gib() {
    let b = bed();
    // A sparse 1 GiB file in the temp folder, older than the minimum age.
    let f = b.ctx.env.temp_dir.join("huge.tmp");
    fs::create_dir_all(&b.ctx.env.temp_dir).unwrap();
    let file = fs::File::create(&f).unwrap();
    file.set_len(GIB + 1).unwrap();
    drop(file);
    crate::testutil::set_age_hours(&f, 100);
    let sp = cat_of(&b.analyze(), CategoryId::Space).clone();
    assert_eq!(sp.status, Status::Problem);
    assert!(sp.metric("bytes") >= GIB);
}

#[test]
fn speed_lists_high_and_medium_startup_items_and_background_apps() {
    let b = speed_bed();
    let r = b.analyze();
    let sp = cat_of(&r, CategoryId::Speed);
    assert_eq!(sp.status, Status::Problem);
    assert!(sp.fixable);
    let Finding::Startup { items } = &sp.findings[0] else {
        panic!("{:?}", sp.findings)
    };
    let got: Vec<(&str, Impact)> = items.iter().map(|i| (i.id.as_str(), i.impact)).collect();
    assert_eq!(
        got,
        [
            ("xdg:user:slack.desktop", Impact::High),
            ("xdg:user:spotify.desktop", Impact::Medium),
        ],
        "not listed: low impact, security software, critical, already disabled"
    );
    let Finding::BackgroundApps { apps } = &sp.findings[1] else {
        panic!("{:?}", sp.findings)
    };
    let ids: Vec<(&str, u64)> = apps
        .iter()
        .map(|a| (a.app_id.as_str(), a.memory_bytes))
        .collect();
    // largest first; firefox (no startup item), the security tool and the tiny tool are not "background"
    assert_eq!(ids[0], ("slack", 300 * MB));
    assert_eq!(ids[1], ("spotify", 60 * MB));
    assert!(
        ids.iter()
            .all(|(id, _)| !["firefox", "mbam", "olddisabled"].contains(id)),
        "{ids:?}"
    );
    assert_eq!(sp.metric("highImpact"), 1);
    assert_eq!(sp.metric("mediumImpact"), 1);
    assert_eq!(sp.metric("startupItems"), 2);
    assert_eq!(
        sp.metric("backgroundMemoryBytes"),
        ids.iter().map(|x| x.1).sum::<u64>()
    );
}

#[test]
fn speed_is_good_on_a_quiet_machine_and_sleeping_apps_are_not_listed() {
    let b = bed();
    assert_eq!(cat_of(&b.analyze(), CategoryId::Speed).status, Status::Good);

    let b = speed_bed();
    b.call("optimizer.sleep", json!({"appIds": ["slack"]}))
        .unwrap();
    let sp = cat_of(&b.analyze(), CategoryId::Speed).clone();
    for f in &sp.findings {
        match f {
            Finding::Startup { items } => assert!(items.iter().all(|i| !i.id.contains("slack"))),
            Finding::BackgroundApps { apps } => assert!(apps.iter().all(|a| a.app_id != "slack")),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn security_counts_updates_and_security_updates_separately() {
    let b = bed();
    script_apt(&b.mock);
    let sp = cat_of(&b.analyze(), CategoryId::Security).clone();
    assert_eq!(sp.status, Status::Problem);
    assert_eq!(sp.metric("updates"), 3);
    assert_eq!(sp.metric("securityUpdates"), 1);
    assert!(sp.fixable);
    let Finding::Updates {
        count,
        security,
        items,
    } = &sp.findings[0]
    else {
        panic!()
    };
    assert_eq!((*count, *security), (3, 1));
    assert_eq!(items[0].id, "apt:firefox", "security updates come first");
    assert!(items[0].security && !items[1].security);
    assert_eq!(sp.summary, "3 updates available, 1 security.");
    // listed without refreshing the package index: only `apt list`, never `apt-get update`
    assert_eq!(
        b.mock.calls(),
        vec![(
            "apt".to_string(),
            vec!["list".to_string(), "--upgradable".to_string()]
        )]
    );
}

#[test]
fn security_ignores_updates_the_user_ignored_and_is_good_without_updates() {
    let b = bed();
    script_apt(&b.mock);
    settings::update(&b.ctx, |s| {
        s.ignored_updates = vec!["apt:firefox".into(), "apt:vim".into(), "apt:htop".into()];
    })
    .unwrap();
    let sp = cat_of(&b.analyze(), CategoryId::Security).clone();
    assert_eq!(sp.status, Status::Good);
    assert!(!sp.fixable);
    assert_eq!(sp.summary, "All software is up to date.");
}

#[test]
fn security_is_unavailable_without_package_tooling() {
    let b = bed();
    let r = b.analyze();
    let sp = cat_of(&r, CategoryId::Security);
    assert_eq!(sp.status, Status::Unavailable);
    assert!(
        sp.summary.contains("No supported package manager"),
        "{}",
        sp.summary
    );
    assert!(!sp.fixable && sp.findings.is_empty());
    // and the score still exists, computed from the three other categories
    assert_eq!(r.score, Some(100));
}

// ================================================================ independence

#[test]
fn a_failing_category_is_unavailable_and_the_others_are_reported() {
    let b = bed();
    b.chrome();
    b.fx.populate_linux_system();
    // Every package manager that is present fails.
    b.mock.on(
        "apt",
        &["list", "--upgradable"],
        CmdOutput::failed(100, "E: broken"),
    );
    let r = b.analyze();
    let sec = cat_of(&r, CategoryId::Security);
    assert_eq!(sec.status, Status::Unavailable);
    assert!(sec.summary.contains("apt list failed"), "{}", sec.summary);
    assert_eq!(cat_of(&r, CategoryId::Privacy).status, Status::Warning);
    assert_eq!(cat_of(&r, CategoryId::Space).status, Status::Warning);
    assert_eq!(cat_of(&r, CategoryId::Speed).status, Status::Good);
    // The score leaves the unavailable category out.
    let want = score::overall(&Inputs {
        security: None,
        ..score::inputs_of(&r.categories)
    });
    assert_eq!(r.score, want);
    assert!(r.score.unwrap() < 100);
}

#[test]
fn the_cleaner_categories_fail_together_and_speed_and_security_survive() {
    let b = bed();
    script_apt(&b.mock);
    // An unusable exclusion makes the cleaner refuse to run (it fails closed).
    write(
        &b.ctx.env.data_dir.join("settings.json"),
        r#"{"exclude":[{"id":"x","pattern":"relative/not-absolute"}]}"#,
    );
    let r = b.analyze();
    for id in [CategoryId::Privacy, CategoryId::Space] {
        let c = cat_of(&r, id);
        assert_eq!(c.status, Status::Unavailable, "{id:?}");
        assert!(c.summary.contains("absolute"), "{}", c.summary);
    }
    assert_eq!(cat_of(&r, CategoryId::Speed).status, Status::Good);
    assert_eq!(cat_of(&r, CategoryId::Security).status, Status::Problem);
    // Only speed (cap 25) and security (cap 30) count: 1 security update (5) + 2 regular (2)
    // of 55 points -> 100 - 7 * 100 / 55 = 87.27
    assert_eq!(r.score, Some(87));
}

#[test]
fn cancellation_is_not_swallowed_as_unavailable() {
    let b = bed();
    let job = Job::detached();
    job.token().cancel();
    let e = analyze::analyze(&b.ctx, &job).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
    assert!(
        store::load(&b.ctx).is_none(),
        "nothing stored for a cancelled scan"
    );
}

#[test]
fn cancelling_mid_scan_stops_before_the_next_category_and_keeps_the_old_last_result() {
    let b = bed();
    script_apt(&b.mock);
    let first = b.analyze();
    let token = CancelToken::new();
    let t2 = token.clone();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let s2 = seen.clone();
    let job = Job::new(token, move |e| {
        if let Some(m) = &e.message {
            s2.lock().unwrap().push(m.clone());
            if m == "Checking speed" {
                t2.cancel();
            }
        }
    });
    let e = run_analyze(&b.ctx, &job).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|m| m == "Checking privacy"));
    assert!(!seen.iter().any(|m| m.starts_with("Checking security")));
    assert_eq!(store::load(&b.ctx).unwrap(), first);
}

#[test]
fn analysis_reports_progress_per_category() {
    let b = bed();
    script_apt(&b.mock);
    let events = Arc::new(Mutex::new(Vec::<ProgressEvent>::new()));
    let e2 = events.clone();
    let job = Job::new(CancelToken::new(), move |e| e2.lock().unwrap().push(e));
    run_analyze(&b.ctx, &job).unwrap();
    let events = events.lock().unwrap();
    for word in ["privacy", "space", "speed", "security"] {
        assert!(
            events
                .iter()
                .any(|e| e.message.as_deref() == Some(&format!("Checking {word}"))),
            "no progress for {word}"
        );
    }
    let last = events.last().unwrap();
    assert_eq!(last.fraction, Some(1.0));
    // Inner progress is scaled into the category's slice, so the bar never runs backwards
    // beyond what the forwarding thread's ordering allows: every fraction stays in 0..=1.
    assert!(events
        .iter()
        .all(|e| e.fraction.is_none_or(|f| (0.0..=1.0).contains(&f))));
}

// ================================================================ last result

#[test]
fn last_result_is_stored_and_read_back() {
    let b = bed();
    script_apt(&b.mock);
    assert_eq!(b.call("health.last", json!({})).unwrap(), Value::Null);
    let r = b.call("health.analyze", json!({})).unwrap();
    assert!(b.ctx.env.data_dir.join("health-last.json").is_file());
    let last = b.call("health.last", Value::Null).unwrap();
    assert_eq!(
        last, r,
        "health.last returns exactly what the scan returned"
    );
    // a newer scan replaces it
    b.fx.populate_linux_system();
    let r2 = b.call("health.analyze", json!({})).unwrap();
    assert_ne!(r2["categories"], r["categories"]);
    assert_eq!(b.call("health.last", json!({})).unwrap(), r2);
}

#[test]
fn a_damaged_last_result_reads_as_never_scanned() {
    let b = bed();
    write(&b.ctx.env.data_dir.join("health-last.json"), "{ not json");
    assert_eq!(b.call("health.last", json!({})).unwrap(), Value::Null);
    write(
        &b.ctx.env.data_dir.join("health-last.json"),
        r#"{"score": "high"}"#,
    );
    assert_eq!(b.call("health.last", json!({})).unwrap(), Value::Null);
    // and a scan simply overwrites it
    b.analyze();
    assert!(b.call("health.last", json!({})).unwrap().is_object());
}

// ================================================================ fix: only what is listed

fn snapshot_files(paths: &[&Path]) -> Vec<Vec<u8>> {
    paths.iter().map(|p| fs::read(p).unwrap()).collect()
}

#[test]
fn fix_space_only_never_touches_cookies_startup_or_updates() {
    let b = bed();
    b.autostart("slack.desktop", &desktop("Slack", "/opt/Slack/slack -u %U"));
    let chrome = b.chrome();
    let sys = b.fx.populate_linux_system();
    b.set_keep(&["google.com"]);
    script_apt(&b.mock);
    let cookies = chrome.data.join("Network/Cookies");
    let history = chrome.data.join("History");
    let slack = crate::features::startup::linux::user_autostart_dir(&b.ctx).join("slack.desktop");
    let before = snapshot_files(&[&cookies, &history, &slack]);
    let junk_before = cat_of(&b.analyze(), CategoryId::Space).metric("bytes");
    assert!(junk_before > 0);

    let r = b.fix(json!({"space": true}));

    assert_eq!(r.parts.len(), 1);
    let p = part(&r, Part::Space);
    assert_eq!(p.status, PartStatus::Done, "{}", p.message);
    assert!(
        p.removed_bytes >= junk_before,
        "{} < {junk_before}",
        p.removed_bytes
    );
    // junk is gone ...
    assert!(!sys.old_tmp.exists());
    assert!(!chrome.cache.join("Cache/Cache_Data/data_0").exists());
    // ... and everything else is byte-for-byte what it was
    assert_eq!(snapshot_files(&[&cookies, &history, &slack]), before);
    assert_eq!(query_i64(&cookies, "SELECT COUNT(*) FROM cookies"), 7);
    assert_eq!(b.procs.exit_requests(), Vec::<u32>::new());
    assert_eq!(b.mutating_calls(), Vec::<String>::new());
    // a fresh result comes back and is stored
    let fresh = r.report.expect("fresh report");
    assert_eq!(cat_of(&fresh, CategoryId::Space).metric("bytes"), 0);
    assert_eq!(cat_of(&fresh, CategoryId::Security).metric("updates"), 3);
    assert_eq!(store::load(&b.ctx).unwrap(), fresh);
    assert!(!r.cancelled);
}

#[test]
fn fix_privacy_only_removes_tracking_data_and_keeps_the_keep_list_and_all_junk() {
    let b = bed();
    let chrome = b.chrome();
    let ff = b.fx.populate_firefox("abcd.default-release");
    let sys = b.fx.populate_linux_system();
    b.set_keep(&["google.com", "github.com", "mozilla.org"]);
    let cookies = chrome.data.join("Network/Cookies");
    let history = chrome.data.join("History");
    let before = b.analyze();

    let r = b.fix(json!({"privacy": true}));

    let p = part(&r, Part::Privacy);
    assert_eq!(p.status, PartStatus::Done, "{}", p.message);
    assert!(p.removed_rows > 0);
    assert_eq!(p.removed_files, 0);
    // kept sites stay signed in: 3 google + 1 github
    assert_eq!(query_i64(&cookies, "SELECT COUNT(*) FROM cookies"), 4);
    assert_eq!(
        query_i64(
            &cookies,
            "SELECT COUNT(*) FROM cookies WHERE host_key LIKE '%tracker%'"
        ),
        0
    );
    assert_eq!(query_i64(&history, "SELECT COUNT(*) FROM urls"), 0);
    assert_eq!(query_i64(&history, "SELECT COUNT(*) FROM downloads"), 0);
    assert_eq!(
        query_i64(
            &ff.data.join("cookies.sqlite"),
            "SELECT COUNT(*) FROM moz_cookies"
        ),
        2
    );
    // no junk file was touched
    assert!(chrome.cache.join("Cache/Cache_Data/data_0").exists());
    assert!(sys.old_tmp.exists() && sys.apt_deb.exists());
    assert!(chrome.data.join("Bookmarks").exists());
    let fresh = r.report.unwrap();
    let p = cat_of(&fresh, CategoryId::Privacy);
    assert_eq!((p.metric("trackers"), p.metric("historyRows")), (0, 0));
    assert_eq!(p.status, Status::Good);
    assert_eq!(
        cat_of(&fresh, CategoryId::Space).metric("bytes"),
        cat_of(&before, CategoryId::Space).metric("bytes"),
        "junk is unchanged"
    );
    assert!(fresh.score.unwrap() >= before.score.unwrap());
}

#[test]
fn fix_privacy_and_space_raises_the_score() {
    let b = bed();
    b.chrome();
    b.fx.populate_linux_system();
    b.set_keep(&["google.com"]);
    let before = b.analyze();
    let r = b.fix(json!({"privacy": true, "space": true}));
    assert_eq!(
        r.parts.iter().map(|p| p.part).collect::<Vec<_>>(),
        [Part::Privacy, Part::Space]
    );
    let after = r.report.unwrap();
    assert!(after.score.unwrap() > before.score.unwrap() || before.score == Some(100));
    assert_eq!(cat_of(&after, CategoryId::Space).status, Status::Good);
    assert_eq!(cat_of(&after, CategoryId::Privacy).status, Status::Good);
    // both parts write history entries labelled as coming from the health check
    let h = cleaner::history::list(&b.ctx, None);
    assert_eq!(h.len(), 2);
    assert!(h.iter().all(|e| e.source == cleaner::Source::Health));
}

#[test]
fn fix_reports_a_running_browser_instead_of_touching_it() {
    let b = bed_with(
        vec![proc(500, "chrome", "/opt/google/chrome/chrome", 200)],
        true,
    );
    let chrome = b.chrome();
    let cookies = chrome.data.join("Network/Cookies");
    let before = fs::read(&cookies).unwrap();
    // The default is "skip": the browser is left alone and reported.
    let r = b.fix(json!({"privacy": true}));
    let p = part(&r, Part::Privacy);
    assert_eq!(p.status, PartStatus::Failed, "{}", p.message);
    assert_eq!(p.blocked_apps, ["Google Chrome"]);
    assert!(p.message.contains("Google Chrome"), "{}", p.message);
    assert_eq!(fs::read(&cookies).unwrap(), before);
    assert_eq!(
        b.procs.exit_requests(),
        Vec::<u32>::new(),
        "nothing was asked to quit"
    );
    // "ask" cannot be answered by the server: it behaves like "skip"
    let r = b.fix(json!({"privacy": true, "closeApps": "ask"}));
    assert_eq!(part(&r, Part::Privacy).blocked_apps, ["Google Chrome"]);
    assert_eq!(fs::read(&cookies).unwrap(), before);

    // The retry the UI offers: close the browser (politely) and fix.
    let r = b.fix(json!({"privacy": true, "closeApps": "always"}));
    let p = part(&r, Part::Privacy);
    assert_eq!(p.status, PartStatus::Done, "{}", p.message);
    assert!(p.blocked_apps.is_empty());
    assert_eq!(b.procs.exit_requests(), vec![500]);
    assert_eq!(query_i64(&cookies, "SELECT COUNT(*) FROM cookies"), 0);
}

#[test]
fn fix_startup_disables_only_the_listed_valid_items() {
    let b = speed_bed();
    let user = crate::features::startup::linux::user_autostart_dir(&b.ctx);
    let slack = user.join("slack.desktop");
    let spotify = user.join("spotify.desktop");
    let tiny = user.join("tiny.desktop");
    let spotify_before = fs::read(&spotify).unwrap();
    let polkit = crate::features::startup::linux::system_autostart_dir(&b.ctx)
        .join("polkit-gnome-authentication-agent-1.desktop");
    let polkit_before = fs::read(&polkit).unwrap();

    let r = b.fix(json!({"startupIds": [
        "xdg:user:slack.desktop",
        "xdg:system:polkit-gnome-authentication-agent-1.desktop",
        "xdg:user:nope.desktop",
        "xdg:user:old.desktop",
        "xdg:user:slack.desktop",
    ]}));

    let p = part(&r, Part::Startup);
    assert_eq!(p.status, PartStatus::Partial, "{}", p.message);
    assert_eq!(p.items.len(), 4, "duplicates are collapsed: {:?}", p.items);
    let by = |id: &str| p.items.iter().find(|i| i.id == id).unwrap();
    assert!(by("xdg:user:slack.desktop").ok);
    let critical = by("xdg:system:polkit-gnome-authentication-agent-1.desktop");
    assert!(
        !critical.ok && critical.message.starts_with("Refused"),
        "{critical:?}"
    );
    assert!(by("xdg:user:nope.desktop")
        .message
        .contains("no longer exists"));
    assert!(by("xdg:user:old.desktop").ok, "already off: nothing to do");
    // on disk
    assert!(fs::read_to_string(&slack).unwrap().contains("Hidden=true"));
    assert_eq!(
        fs::read(&spotify).unwrap(),
        spotify_before,
        "unlisted items are untouched"
    );
    assert!(!fs::read_to_string(&tiny).unwrap().contains("Hidden"));
    assert_eq!(fs::read(&polkit).unwrap(), polkit_before);
    assert_eq!(
        b.procs.exit_requests(),
        Vec::<u32>::new(),
        "processes are not stopped here"
    );
    // the fresh result no longer offers slack
    let fresh = r.report.unwrap();
    let Finding::Startup { items } = &cat_of(&fresh, CategoryId::Speed).findings[0] else {
        panic!()
    };
    assert_eq!(
        items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        ["xdg:user:spotify.desktop"]
    );
}

#[test]
fn fix_startup_refuses_services_and_never_calls_systemctl() {
    let b = bed();
    b.mock.on(
        "systemctl",
        crate::features::startup::linux::SYSTEMCTL_USER_LIST,
        CmdOutput::ok("syncthing.service enabled enabled\n"),
    );
    b.mock.on(
        "systemctl",
        crate::features::startup::linux::SYSTEMCTL_SYSTEM_LIST,
        CmdOutput::ok("cups.service enabled enabled\n"),
    );
    let r = b.fix(
        json!({"startupIds": ["systemd:user:syncthing.service", "systemd:system:cups.service"]}),
    );
    let p = part(&r, Part::Startup);
    assert_eq!(p.status, PartStatus::Failed);
    assert!(
        p.items
            .iter()
            .all(|i| !i.ok && i.message.starts_with("Refused")),
        "{:?}",
        p.items
    );
    assert_eq!(b.mutating_calls(), Vec::<String>::new());
}

#[test]
fn fix_sleep_puts_only_listed_background_apps_to_sleep() {
    let b = speed_bed();
    let user = crate::features::startup::linux::user_autostart_dir(&b.ctx);
    let r = b.fix(json!({"sleepAppIds": ["spotify", "firefox", "mbam", "nope"]}));
    let p = part(&r, Part::Sleep);
    assert_eq!(p.status, PartStatus::Partial, "{}", p.message);
    let by = |id: &str| {
        p.items
            .iter()
            .find(|i| i.id == id)
            .unwrap_or_else(|| panic!("{id}: {:?}", p.items))
    };
    assert!(by("spotify").ok, "{:?}", by("spotify"));
    for id in ["firefox", "mbam", "nope"] {
        assert!(
            !by(id).ok && by(id).message.starts_with("Refused"),
            "{:?}",
            by(id)
        );
    }
    // only Spotify's process was asked to quit, only its startup item was switched off
    assert_eq!(b.procs.exit_requests(), vec![101]);
    assert!(fs::read_to_string(user.join("spotify.desktop"))
        .unwrap()
        .contains("Hidden=true"));
    assert!(!fs::read_to_string(user.join("slack.desktop"))
        .unwrap()
        .contains("Hidden"));
    let state: Value = serde_json::from_str(
        &fs::read_to_string(b.ctx.env.data_dir.join("optimizer.json")).unwrap(),
    )
    .unwrap();
    assert!(state["sleeping"]["spotify"].is_object());
    assert!(state["sleeping"].get("firefox").is_none());
}

#[test]
fn fix_updates_installs_only_listed_updates_and_refuses_ignored_and_unknown_ids() {
    let b = bed();
    script_apt(&b.mock);
    b.mock.on(
        "apt-get",
        &["install", "--only-upgrade", "-y", "firefox"],
        CmdOutput::ok("done"),
    );
    settings::update(&b.ctx, |s| s.ignored_updates = vec!["apt:vim".into()]).unwrap();
    let r = with_elevation(true, || {
        b.fix(json!({"updateIds": ["apt:firefox", "apt:vim", "apt:ghost"]}))
    });
    let p = part(&r, Part::Updates);
    assert_eq!(p.status, PartStatus::Partial, "{}", p.message);
    let by = |id: &str| p.items.iter().find(|i| i.id == id).unwrap();
    assert!(by("apt:firefox").ok);
    assert!(
        by("apt:vim").message.contains("ignore"),
        "{:?}",
        by("apt:vim")
    );
    assert!(!by("apt:ghost").ok && by("apt:ghost").message.contains("no update"));
    let installs: Vec<String> = b
        .mock
        .calls()
        .into_iter()
        .filter(|(p, _)| p == "apt-get")
        .map(|(_, a)| a.join(" "))
        .collect();
    assert_eq!(installs, ["install --only-upgrade -y firefox"]);
}

#[test]
fn fix_runs_the_parts_in_a_fixed_order_and_each_part_on_its_own() {
    let b = speed_bed();
    b.chrome();
    script_apt(&b.mock);
    b.mock.on(
        "apt-get",
        &["install", "--only-upgrade", "-y", "vim"],
        CmdOutput::ok(""),
    );
    let r = with_elevation(true, || {
        b.fix(json!({
            "updateIds": ["apt:vim"],
            "sleepAppIds": ["spotify"],
            "startupIds": ["xdg:user:slack.desktop"],
            "space": true,
            "privacy": true,
        }))
    });
    assert_eq!(
        r.parts.iter().map(|p| p.part).collect::<Vec<_>>(),
        [
            Part::Privacy,
            Part::Space,
            Part::Startup,
            Part::Sleep,
            Part::Updates
        ]
    );
    // (the fake machine runs Firefox and Slack, so the cleaner leaves their data alone: partial)
    for p in &r.parts {
        assert_ne!(p.status, PartStatus::Failed, "{p:?}");
        assert_ne!(p.status, PartStatus::NotRun, "{p:?}");
    }
    for which in [Part::Startup, Part::Sleep, Part::Updates] {
        assert_eq!(
            part(&r, which).status,
            PartStatus::Done,
            "{:?}",
            part(&r, which)
        );
    }
}

#[test]
fn fix_progress_names_each_part() {
    let b = bed();
    b.chrome();
    let msgs = Arc::new(Mutex::new(Vec::<String>::new()));
    let m2 = msgs.clone();
    let job = Job::new(CancelToken::new(), move |e| {
        if let Some(m) = e.message {
            m2.lock().unwrap().push(m);
        }
    });
    dispatch(
        &b.ctx,
        "health.fix",
        json!({"privacy": true, "space": true}),
        &job,
    )
    .unwrap();
    let msgs = msgs.lock().unwrap();
    let pos = |m: &str| {
        msgs.iter()
            .position(|x| x == m)
            .unwrap_or_else(|| panic!("{m} not in {msgs:?}"))
    };
    assert!(pos("Removing tracking data") < pos("Deleting junk files"));
    assert!(pos("Deleting junk files") < pos("Re-checking"));
}

// ================================================================ fix: cancellation

#[test]
fn a_cancelled_fix_stops_between_parts_and_forgets_the_stale_result() {
    let b = speed_bed();
    let chrome = b.chrome();
    let sys = b.fx.populate_linux_system();
    b.set_keep(&["google.com"]);
    b.analyze();
    assert!(store::load(&b.ctx).is_some());
    let slack = crate::features::startup::linux::user_autostart_dir(&b.ctx).join("slack.desktop");
    let slack_before = fs::read(&slack).unwrap();

    let token = CancelToken::new();
    let t2 = token.clone();
    let job = Job::new(token, move |e| {
        // The user presses Cancel as the third part begins.
        if e.message.as_deref() == Some("Switching off startup items") {
            t2.cancel();
        }
    });
    let req = FixRequest {
        privacy: true,
        space: true,
        startup_ids: vec!["xdg:user:slack.desktop".into()],
        sleep_app_ids: vec!["spotify".into()],
        ..FixRequest::default()
    };
    let r = fix::fix(&b.ctx, &req, &job).unwrap();

    assert!(r.cancelled);
    assert!(r.report.is_none());
    assert_ne!(part(&r, Part::Privacy).status, PartStatus::NotRun);
    assert!(part(&r, Part::Privacy).removed_rows > 0);
    assert_ne!(part(&r, Part::Space).status, PartStatus::NotRun);
    assert_eq!(part(&r, Part::Startup).status, PartStatus::Partial);
    assert_eq!(part(&r, Part::Sleep).status, PartStatus::NotRun);
    // the first two parts really happened, the rest did not
    assert!(!sys.old_tmp.exists());
    assert_eq!(
        query_i64(
            &chrome.data.join("Network/Cookies"),
            "SELECT COUNT(*) FROM cookies"
        ),
        3
    );
    assert_eq!(fs::read(&slack).unwrap(), slack_before);
    assert_eq!(b.procs.exit_requests(), Vec::<u32>::new());
    assert!(
        store::load(&b.ctx).is_none(),
        "the stored result no longer describes the machine"
    );
}

#[test]
fn a_fix_cancelled_before_it_starts_changes_nothing() {
    let b = bed();
    let sys = b.fx.populate_linux_system();
    let job = Job::detached();
    job.token().cancel();
    let req = FixRequest {
        space: true,
        privacy: true,
        ..FixRequest::default()
    };
    let r = fix::fix(&b.ctx, &req, &job).unwrap();
    assert!(r.cancelled && r.report.is_none());
    assert!(r.parts.iter().all(|p| p.status == PartStatus::NotRun));
    assert!(sys.old_tmp.exists());
    // through the dispatcher a pre-cancelled call is refused outright
    let e = dispatch(&b.ctx, "health.fix", json!({"space": true}), &job).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
}

// ================================================================ parameter validation

#[test]
fn fix_needs_something_to_do_and_rejects_unknown_or_oversized_input() {
    let b = bed();
    for params in [
        json!({}),
        Value::Null,
        json!({"privacy": false, "space": false}),
        json!({"startupIds": [], "sleepAppIds": [], "updateIds": []}),
    ] {
        let e = b.call("health.fix", params.clone()).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams, "{params}");
    }
    for params in [
        json!({"space": true, "paths": ["/etc"]}),
        json!({"space": true, "ruleIds": ["chrome.cache"]}),
        json!({"space": "yes"}),
        json!({"startupIds": [""]}),
        json!({"startupIds": ["x".repeat(301)]}),
        json!({"updateIds": (0..501).map(|i| format!("apt:p{i}")).collect::<Vec<_>>()}),
        json!({"space": true, "closeApps": "kill"}),
    ] {
        let e = b.call("health.fix", params.clone()).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams, "{params}");
    }
    assert_eq!(b.mutating_calls(), Vec::<String>::new());
}

#[test]
fn methods_are_registered_and_stubs_are_gone() {
    for m in METHODS {
        assert!(crate::api::registry().get(m).is_some(), "{m}");
    }
    assert_eq!(METHODS, &["health.analyze", "health.last", "health.fix"]);
    let b = bed();
    let e = b.call("health.analyze", json!({})).map(|_| ()).err();
    assert!(e.is_none());
}

#[test]
fn nothing_is_modified_by_analysis() {
    let b = speed_bed();
    b.chrome();
    b.fx.populate_linux_system();
    script_apt(&b.mock);
    let root = b._tmp.path().to_path_buf();
    let strip = |mut m: std::collections::BTreeMap<String, String>| {
        // our own bookkeeping files are the only allowed change
        m.retain(|k, _| !k.starts_with("data/"));
        m
    };
    let before = strip(crate::testutil::tree_snapshot(&root));
    b.analyze();
    assert_eq!(strip(crate::testutil::tree_snapshot(&root)), before);
    assert_eq!(b.mutating_calls(), Vec::<String>::new());
}
