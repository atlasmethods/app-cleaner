//! Startup manager tests. Platform logic is exercised on any host: `ctx.env.os` selects the
//! platform, systemctl / crontab / schtasks / reg / launchctl are scripted, and the Windows
//! registry is a fake.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::model::{Entry, Impact, Kind, Scope, Target, WinHive};
use super::windows::{self as win, RegData, StartupRegistry};
use super::*;
use crate::ctx::{Ctx, Os};
use crate::elevate::{powershell_decode, with_elevation};
use crate::error::ErrorCode;
use crate::job::Job;
use crate::procs::{FakeProcesses, ProcDetail};
use crate::runner::{CmdOutput, CommandRunner, MockRunner};

// ---------------------------------------------------------------- helpers

fn job() -> Job {
    Job::detached()
}

fn ctx_os(dir: &Path, os: Os, m: &MockRunner) -> Ctx {
    let mut c = Ctx::test(dir, m.clone()).with_procs(Arc::new(FakeProcesses::new(&[], true)));
    c.env.os = os;
    c
}

fn linux_ctx(dir: &Path) -> (Ctx, MockRunner) {
    let m = MockRunner::new();
    (ctx_os(dir, Os::Linux, &m), m)
}

fn write(p: &Path, text: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn autostart(ctx: &Ctx) -> PathBuf {
    linux::user_autostart_dir(ctx)
}

fn sys_autostart(ctx: &Ctx) -> PathBuf {
    linux::system_autostart_dir(ctx)
}

fn item<'a>(entries: &'a [Entry], id: &str) -> &'a Entry {
    entries.iter().find(|e| e.item.id == id).unwrap_or_else(|| {
        panic!(
            "no item {id}; have {:?}",
            entries.iter().map(|e| &e.item.id).collect::<Vec<_>>()
        )
    })
}

/// A `crontab` that keeps its table in memory and reads new tables from the file argument,
/// and forwards everything else to a `MockRunner`.
#[derive(Clone)]
struct Sim {
    mock: MockRunner,
    cron: Arc<Mutex<Option<String>>>,
    installs: Arc<Mutex<u32>>,
}

impl Sim {
    fn new(mock: &MockRunner, cron: Option<&str>) -> Sim {
        Sim {
            mock: mock.clone(),
            cron: Arc::new(Mutex::new(cron.map(str::to_string))),
            installs: Arc::new(Mutex::new(0)),
        }
    }
    fn table(&self) -> Option<String> {
        self.cron.lock().unwrap().clone()
    }
}

impl CommandRunner for Sim {
    fn run(&self, program: &str, args: &[&str]) -> crate::error::Result<CmdOutput> {
        if program == "crontab" {
            if args == ["-l"] {
                return Ok(match self.cron.lock().unwrap().clone() {
                    Some(t) => CmdOutput::ok(t),
                    None => CmdOutput::failed(1, "no crontab for user"),
                });
            }
            let text = fs::read_to_string(args[0]).unwrap();
            *self.cron.lock().unwrap() = Some(text);
            *self.installs.lock().unwrap() += 1;
            return Ok(CmdOutput::ok(""));
        }
        self.mock.run(program, args)
    }
    fn which(&self, program: &str) -> Option<PathBuf> {
        if program == "crontab" {
            return Some(PathBuf::from("/mock/bin/crontab"));
        }
        self.mock.which(program)
    }
}

fn sim_ctx(dir: &Path, os: Os, sim: &Sim) -> Ctx {
    let mut c = Ctx::new(crate::ctx::Env::for_test(dir), Arc::new(sim.clone()))
        .with_procs(Arc::new(FakeProcesses::new(&[], true)));
    c.env.os = os;
    c
}

// ---------------------------------------------------------------- desktop files

const SLACK: &str =
    "[Desktop Entry]\nType=Application\nName=Slack\nExec=/usr/bin/slack -u %U\nIcon=slack\n";

#[test]
fn desktop_parse_and_enabled_semantics() {
    let d = linux::parse_desktop(
        "[Desktop Entry]\nName=A\nName[de]=B\nExec=/bin/a %f\nOnlyShowIn=GNOME;\nNotShowIn=KDE;\n[Desktop Action x]\nName=Other\nExec=/bin/other\n",
    );
    assert_eq!(d.name.as_deref(), Some("A"));
    assert_eq!(d.exec.as_deref(), Some("/bin/a %f"));
    assert!(
        d.enabled(),
        "OnlyShowIn / NotShowIn do not affect the state"
    );
    assert!(!linux::parse_desktop("[Desktop Entry]\nExec=x\nHidden=true\n").enabled());
    assert!(
        !linux::parse_desktop("[Desktop Entry]\nExec=x\nX-GNOME-Autostart-enabled=false\n")
            .enabled()
    );
    assert!(linux::parse_desktop(
        "[Desktop Entry]\nExec=x\nX-GNOME-Autostart-enabled=true\nHidden=false\n"
    )
    .enabled());
    // keys outside [Desktop Entry] are ignored
    assert!(linux::parse_desktop("[Desktop Entry]\nExec=x\n[Other]\nHidden=true\n").enabled());
}

#[test]
fn set_hidden_then_clear_is_byte_exact() {
    let cases = [
        SLACK.to_string(),
        // no trailing newline
        "[Desktop Entry]\nName=Slack\nExec=/usr/bin/slack".to_string(),
        // CRLF
        "[Desktop Entry]\r\nName=Slack\r\nExec=/usr/bin/slack\r\n".to_string(),
        // more groups, blank lines, comments, odd spacing
        "# comment\n[Desktop Entry]\nName = Slack\nExec=/usr/bin/slack\n\n\n[Desktop Action a]\nName=A\nExec=/bin/a\n"
            .to_string(),
        // trailing blank lines
        "[Desktop Entry]\nName=Slack\nExec=x\n\n\n".to_string(),
        // non-ASCII
        "[Desktop Entry]\nName=Slåck ✓\nExec=/usr/bin/slack\n".to_string(),
    ];
    for original in cases {
        let hidden = linux::set_hidden(&original).unwrap();
        assert!(linux::parse_desktop(&hidden).hidden, "{original:?}");
        assert!(!linux::parse_desktop(&hidden).enabled());
        assert_ne!(hidden, original);
        // Hiding twice changes nothing more.
        assert_eq!(linux::set_hidden(&hidden).unwrap(), hidden);
        assert_eq!(
            linux::clear_hidden(&hidden),
            original,
            "round trip of {original:?}"
        );
    }
}

#[test]
fn set_hidden_only_touches_the_desktop_entry_group_and_keeps_other_bytes() {
    let original = "[Desktop Entry]\nName=Slack\nExec=x\n\n[Desktop Action a]\nName=A\n";
    let hidden = linux::set_hidden(original).unwrap();
    assert_eq!(
        hidden,
        "[Desktop Entry]\nName=Slack\nExec=x\nHidden=true\n\n[Desktop Action a]\nName=A\n"
    );
}

#[test]
fn set_hidden_replaces_an_existing_hidden_false_and_rejects_non_desktop_files() {
    let hidden = linux::set_hidden("[Desktop Entry]\nHidden=false\nExec=x\n").unwrap();
    assert_eq!(hidden, "[Desktop Entry]\nHidden=true\nExec=x\n");
    assert!(linux::set_hidden("just some text\n").is_err());
    // Clearing also undoes X-GNOME-Autostart-enabled=false, but keeps =true.
    assert_eq!(
        linux::clear_hidden("[Desktop Entry]\nExec=x\nX-GNOME-Autostart-enabled=false\n"),
        "[Desktop Entry]\nExec=x\n"
    );
    assert_eq!(
        linux::clear_hidden("[Desktop Entry]\nExec=x\nX-GNOME-Autostart-enabled=true\n"),
        "[Desktop Entry]\nExec=x\nX-GNOME-Autostart-enabled=true\n"
    );
}

#[test]
fn xdg_user_entry_disable_and_enable_restore_the_file_exactly() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, _m) = linux_ctx(d.path());
    let file = autostart(&ctx).join("slack.desktop");
    write(&file, SLACK);

    let entries = collect(&ctx, &job()).unwrap();
    let e = item(&entries, "xdg:user:slack.desktop");
    assert_eq!(e.item.name, "Slack");
    assert_eq!(e.item.command, "/usr/bin/slack -u %U");
    assert_eq!(e.item.kind, Kind::Autostart);
    assert_eq!(e.item.scope, Scope::User);
    assert!(e.item.enabled && e.item.can_disable && e.item.can_delete && !e.item.critical);
    assert_eq!(e.exe.as_deref(), Some("/usr/bin/slack"));

    let it = set_enabled_by_id(
        &ctx,
        &job(),
        "xdg:user:slack.desktop",
        false,
        ToggleOpts::default(),
    )
    .unwrap();
    assert!(!it.enabled);
    let after = fs::read_to_string(&file).unwrap();
    assert_eq!(after, format!("{SLACK}Hidden=true\n"));

    let it = set_enabled_by_id(
        &ctx,
        &job(),
        "xdg:user:slack.desktop",
        true,
        ToggleOpts::default(),
    )
    .unwrap();
    assert!(it.enabled);
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        SLACK,
        "byte-exact restore"
    );
}

#[test]
fn xdg_system_entry_is_disabled_by_a_user_override_and_reenabled_by_removing_it() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, _m) = linux_ctx(d.path());
    let sys = sys_autostart(&ctx).join("blueman.desktop");
    let sys_text = "[Desktop Entry]\nType=Application\nName=Blueman Applet\nExec=blueman-applet\nOnlyShowIn=GNOME;\n";
    write(&sys, sys_text);

    let entries = collect(&ctx, &job()).unwrap();
    let e = item(&entries, "xdg:system:blueman.desktop");
    assert_eq!(e.item.scope, Scope::System);
    assert!(e.item.enabled && e.item.can_disable);
    assert!(!e.item.can_delete, "a system file cannot be deleted");

    set_enabled_by_id(
        &ctx,
        &job(),
        "xdg:system:blueman.desktop",
        false,
        ToggleOpts::default(),
    )
    .unwrap();
    // The system file is untouched; the override is the same name in the user dir.
    assert_eq!(fs::read_to_string(&sys).unwrap(), sys_text);
    let over = autostart(&ctx).join("blueman.desktop");
    let over_text = fs::read_to_string(&over).unwrap();
    assert!(over_text.contains("Hidden=true"));
    assert!(over_text.contains("Exec=blueman-applet"));

    let entries = collect(&ctx, &job()).unwrap();
    let e = item(&entries, "xdg:system:blueman.desktop");
    assert!(!e.item.enabled);
    assert_eq!(e.item.name, "Blueman Applet");

    set_enabled_by_id(
        &ctx,
        &job(),
        "xdg:system:blueman.desktop",
        true,
        ToggleOpts::default(),
    )
    .unwrap();
    assert!(!over.exists(), "the plain override is deleted");
    assert_eq!(fs::read_to_string(&sys).unwrap(), sys_text);
    assert!(
        item(
            &collect(&ctx, &job()).unwrap(),
            "xdg:system:blueman.desktop"
        )
        .item
        .enabled
    );
}

#[test]
fn a_customised_user_override_survives_reenabling() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, _m) = linux_ctx(d.path());
    write(
        &sys_autostart(&ctx).join("tool.desktop"),
        "[Desktop Entry]\nName=Tool\nExec=tool\n",
    );
    let over = autostart(&ctx).join("tool.desktop");
    write(
        &over,
        "[Desktop Entry]\nName=Tool\nExec=tool --my-flag\nHidden=true\n",
    );
    let entries = collect(&ctx, &job()).unwrap();
    let e = item(&entries, "xdg:system:tool.desktop");
    assert!(!e.item.enabled);
    assert_eq!(e.item.command, "tool --my-flag");
    set_enabled_by_id(
        &ctx,
        &job(),
        "xdg:system:tool.desktop",
        true,
        ToggleOpts::default(),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(&over).unwrap(),
        "[Desktop Entry]\nName=Tool\nExec=tool --my-flag\n"
    );
}

#[test]
fn user_stub_override_without_exec_uses_the_system_entry_for_display() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, _m) = linux_ctx(d.path());
    write(
        &sys_autostart(&ctx).join("agent.desktop"),
        "[Desktop Entry]\nName=Agent\nExec=agent-bin\n",
    );
    write(
        &autostart(&ctx).join("agent.desktop"),
        "[Desktop Entry]\nHidden=true\n",
    );
    let entries = collect(&ctx, &job()).unwrap();
    let e = item(&entries, "xdg:system:agent.desktop");
    assert_eq!(e.item.name, "Agent");
    assert!(!e.item.enabled);
    // Re-enabling removes the stub.
    set_enabled_by_id(
        &ctx,
        &job(),
        "xdg:system:agent.desktop",
        true,
        ToggleOpts::default(),
    )
    .unwrap();
    assert!(!autostart(&ctx).join("agent.desktop").exists());
}

#[test]
fn critical_autostart_entries_are_refused() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, _m) = linux_ctx(d.path());
    let f = sys_autostart(&ctx).join("polkit-gnome-authentication-agent-1.desktop");
    write(&f, "[Desktop Entry]\nName=PolicyKit Agent\nExec=/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1\n");
    let entries = collect(&ctx, &job()).unwrap();
    let e = item(
        &entries,
        "xdg:system:polkit-gnome-authentication-agent-1.desktop",
    );
    assert!(e.item.critical && !e.item.can_disable && !e.item.can_delete);
    let err =
        set_enabled_by_id(&ctx, &job(), &e.item.id, false, ToggleOpts::default()).unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(!autostart(&ctx)
        .join("polkit-gnome-authentication-agent-1.desktop")
        .exists());
    let err = remove_entry(&ctx, e).unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
}

// ---------------------------------------------------------------- systemd

const USER_UNITS: &str = "\
syncthing.service                 enabled  enabled
pipewire.service                  enabled  enabled
dbus.service                      disabled enabled
backup-sync.service               disabled disabled
weird@.service                    disabled enabled
foo.service                       static   -
";

const SYSTEM_UNITS: &str = "\
ssh.service                       enabled  enabled
NetworkManager.service            enabled  enabled
systemd-resolved.service          enabled  enabled
cron.service                      enabled  enabled
bluetooth.service                 enabled  enabled
docker.service                    disabled enabled
getty@.service                    enabled  enabled
gdm.service                       enabled  enabled
myapp.service                     enabled  enabled
";

#[test]
fn parses_unit_file_listings() {
    let u = linux::parse_unit_files(USER_UNITS);
    assert_eq!(u.len(), 6);
    assert_eq!(
        u[0],
        ("syncthing.service".to_string(), "enabled".to_string())
    );
    assert_eq!(u[5].1, "static");
    // no legend rows, stray output and non-service units are ignored
    assert!(
        linux::parse_unit_files("UNIT FILE STATE\n\n1 unit files listed.\nx.socket enabled\n")
            .is_empty()
    );
}

#[test]
fn systemd_items_criticality_and_warnings() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, m) = linux_ctx(d.path());
    m.on(
        "systemctl",
        linux::SYSTEMCTL_USER_LIST,
        CmdOutput::ok(USER_UNITS),
    );
    m.on(
        "systemctl",
        linux::SYSTEMCTL_SYSTEM_LIST,
        CmdOutput::ok(SYSTEM_UNITS),
    );
    write(
        &ctx.env.config_dir.join("systemd/user/syncthing.service"),
        "[Service]\nExecStart=-/usr/bin/syncthing serve --no-browser\n",
    );
    let entries = collect(&ctx, &job()).unwrap();

    let s = item(&entries, "systemd:user:syncthing.service");
    assert_eq!(s.item.kind, Kind::Service);
    assert_eq!(s.item.scope, Scope::User);
    assert!(s.item.enabled && s.item.can_disable && !s.item.critical);
    assert_eq!(s.item.command, "/usr/bin/syncthing serve --no-browser");
    assert_eq!(s.exe.as_deref(), Some("/usr/bin/syncthing"));

    assert!(
        item(&entries, "systemd:user:pipewire.service")
            .item
            .critical
    );
    assert!(item(&entries, "systemd:user:dbus.service").item.critical);
    let b = item(&entries, "systemd:user:backup-sync.service");
    assert!(!b.item.enabled && !b.item.critical);
    // static units and template units are not listed
    assert!(!entries.iter().any(|e| e.item.id.contains("foo.service")));
    assert!(!entries.iter().any(|e| e.item.id.contains("@.service")));

    for critical in ["NetworkManager", "systemd-resolved", "cron", "gdm"] {
        let e = item(&entries, &format!("systemd:system:{critical}.service"));
        assert!(e.item.critical && !e.item.can_disable, "{critical}");
    }
    let ssh = item(&entries, "systemd:system:ssh.service");
    assert!(!ssh.item.critical && ssh.item.can_disable);
    assert!(ssh.item.warning.as_deref().unwrap().contains("SSH"));
    assert!(
        !item(&entries, "systemd:system:bluetooth.service")
            .item
            .critical
    );
    assert!(!item(&entries, "systemd:system:myapp.service").item.critical);
    assert_eq!(
        item(&entries, "systemd:system:myapp.service").item.scope,
        Scope::System
    );
}

#[test]
fn systemd_toggle_runs_the_right_commands() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, m) = linux_ctx(d.path());
    m.on(
        "systemctl",
        linux::SYSTEMCTL_USER_LIST,
        CmdOutput::ok(USER_UNITS),
    );
    m.on(
        "systemctl",
        linux::SYSTEMCTL_SYSTEM_LIST,
        CmdOutput::ok(SYSTEM_UNITS),
    );
    m.on(
        "systemctl",
        &["--user", "disable", "syncthing.service"],
        CmdOutput::ok(""),
    );
    m.on(
        "systemctl",
        &["disable", "myapp.service"],
        CmdOutput::ok(""),
    );
    m.on(
        "systemctl",
        &["--user", "enable", "backup-sync.service"],
        CmdOutput::failed(1, "Failed to enable unit: Unit file does not exist"),
    );
    set_enabled_by_id(
        &ctx,
        &job(),
        "systemd:user:syncthing.service",
        false,
        ToggleOpts::default(),
    )
    .unwrap();
    with_elevation(true, || {
        set_enabled_by_id(
            &ctx,
            &job(),
            "systemd:system:myapp.service",
            false,
            ToggleOpts::default(),
        )
        .unwrap();
    });
    let err = set_enabled_by_id(
        &ctx,
        &job(),
        "systemd:user:backup-sync.service",
        true,
        ToggleOpts::default(),
    )
    .unwrap_err();
    assert!(
        err.message.contains("Unit file does not exist"),
        "{}",
        err.message
    );
    let calls = m.calls();
    assert!(calls
        .iter()
        .any(|(p, a)| p == "systemctl" && a == &["--user", "disable", "syncthing.service"]));
    assert!(calls
        .iter()
        .any(|(p, a)| p == "systemctl" && a == &["disable", "myapp.service"]));
    // critical ones are never passed to systemctl
    let err = set_enabled_by_id(
        &ctx,
        &job(),
        "systemd:system:cron.service",
        false,
        ToggleOpts::default(),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(!m
        .calls()
        .iter()
        .any(|(_, a)| a.contains(&"cron.service".to_string())));
}

#[test]
fn system_service_change_is_elevated_with_pkexec_when_not_root() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, m) = linux_ctx(d.path());
    m.on("systemctl", linux::SYSTEMCTL_USER_LIST, CmdOutput::ok(""));
    m.on(
        "systemctl",
        linux::SYSTEMCTL_SYSTEM_LIST,
        CmdOutput::ok(SYSTEM_UNITS),
    );
    m.on(
        "pkexec",
        &["systemctl", "disable", "docker.service"],
        CmdOutput::ok(""),
    );
    // docker is disabled already -> enabling goes through pkexec
    m.on(
        "pkexec",
        &["systemctl", "enable", "docker.service"],
        CmdOutput::ok(""),
    );
    with_elevation(false, || {
        set_enabled_by_id(
            &ctx,
            &job(),
            "systemd:system:docker.service",
            true,
            ToggleOpts::default(),
        )
        .unwrap();
    });
    assert!(m
        .calls()
        .iter()
        .any(|(p, a)| p == "pkexec" && a == &["systemctl", "enable", "docker.service"]));
}

// ---------------------------------------------------------------- cron

const CRON: &str = "# my jobs\nMAILTO=me@example.com\n*/5 * * * * /usr/bin/true\n@reboot /home/u/bin/sync.sh --quiet\n\n  @reboot   /opt/x/run   \n@reboot /home/u/bin/sync.sh --quiet\n@daily /bin/echo hi\n";

#[test]
fn cron_lines_are_recognised() {
    assert_eq!(
        linux::cron_reboot_line("@reboot /bin/x"),
        Some((true, "@reboot /bin/x".into()))
    );
    assert_eq!(
        linux::cron_reboot_line("# clearsweep-disabled: @reboot /bin/x"),
        Some((false, "@reboot /bin/x".into()))
    );
    assert_eq!(linux::cron_reboot_line("@reboot"), None);
    assert_eq!(linux::cron_reboot_line("@rebootx foo"), None);
    assert_eq!(linux::cron_reboot_line("# @reboot /bin/x"), None);
    assert_eq!(linux::cron_reboot_line("@daily /bin/x"), None);
    assert_eq!(linux::cron_reboot_line("0 0 * * * @reboot"), None);
}

#[test]
fn cron_toggle_rewrites_only_the_target_line() {
    let off = linux::cron_toggle(CRON, "@reboot /home/u/bin/sync.sh --quiet", 1, false).unwrap();
    let expected = CRON.replacen(
        "@reboot /home/u/bin/sync.sh --quiet\n@daily",
        "# clearsweep-disabled: @reboot /home/u/bin/sync.sh --quiet\n@daily",
        1,
    );
    assert_eq!(off, expected, "second identical line only");
    let back = linux::cron_toggle(&off, "@reboot /home/u/bin/sync.sh --quiet", 1, true).unwrap();
    assert_eq!(back, CRON, "exact restore");

    // leading/trailing spaces and CRLF are preserved
    let crlf = "@reboot /a\r\n0 1 * * * b\r\n";
    let off = linux::cron_toggle(crlf, "@reboot /a", 0, false).unwrap();
    assert_eq!(off, "# clearsweep-disabled: @reboot /a\r\n0 1 * * * b\r\n");
    assert_eq!(
        linux::cron_toggle(&off, "@reboot /a", 0, true).unwrap(),
        crlf
    );
    let spaced = linux::cron_toggle(CRON, "  @reboot   /opt/x/run   ", 0, false).unwrap();
    assert!(spaced.contains("# clearsweep-disabled:   @reboot   /opt/x/run   \n"));
    assert_eq!(
        linux::cron_toggle(&spaced, "  @reboot   /opt/x/run   ", 0, true).unwrap(),
        CRON
    );
    assert!(linux::cron_toggle(CRON, "@reboot /nope", 0, false).is_err());
    assert!(linux::cron_toggle(CRON, "@reboot /home/u/bin/sync.sh --quiet", 5, false).is_err());
}

#[test]
fn cron_items_toggle_through_crontab_and_leave_other_lines_alone() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let sim = Sim::new(&m, Some(CRON));
    let ctx = sim_ctx(d.path(), Os::Linux, &sim);
    let entries = collect(&ctx, &job()).unwrap();
    let cron: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.item.kind == Kind::Cron)
        .collect();
    assert_eq!(cron.len(), 3);
    assert_eq!(cron[0].item.scope, Scope::User);
    assert!(cron.iter().all(|e| e.item.enabled && e.item.can_delete));
    let ids: std::collections::HashSet<_> = cron.iter().map(|e| e.item.id.clone()).collect();
    assert_eq!(ids.len(), 3, "identical lines get distinct ids");
    let id = cron
        .iter()
        .find(|e| e.item.command.contains("--quiet"))
        .unwrap()
        .item
        .id
        .clone();

    let it = set_enabled_by_id(&ctx, &job(), &id, false, ToggleOpts::default()).unwrap();
    assert!(!it.enabled);
    let t = sim.table().unwrap();
    assert!(t.contains("# clearsweep-disabled: @reboot /home/u/bin/sync.sh --quiet\n"));
    assert!(t.contains("MAILTO=me@example.com\n*/5 * * * * /usr/bin/true\n"));
    // id is stable across the state change
    let entries = collect(&ctx, &job()).unwrap();
    assert!(!item(&entries, &id).item.enabled);
    set_enabled_by_id(&ctx, &job(), &id, true, ToggleOpts::default()).unwrap();
    assert_eq!(sim.table().unwrap(), CRON);
}

#[test]
fn no_crontab_means_no_cron_items_and_missing_program_is_fine() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let sim = Sim::new(&m, None);
    let ctx = sim_ctx(d.path(), Os::Linux, &sim);
    assert!(collect(&ctx, &job())
        .unwrap()
        .iter()
        .all(|e| e.item.kind != Kind::Cron));
    let (ctx2, _m2) = linux_ctx(d.path());
    assert!(collect(&ctx2, &job()).unwrap().is_empty());
}

// ---------------------------------------------------------------- remove + backup + restore

#[test]
fn remove_backs_up_first_and_restore_puts_the_file_back_exactly() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, _m) = linux_ctx(d.path());
    let f = autostart(&ctx).join("slack.desktop");
    write(&f, SLACK);
    let out = dispatch_json(
        &ctx,
        "startup.remove",
        json!({"id": "xdg:user:slack.desktop"}),
    )
    .unwrap();
    assert!(!f.exists());
    let bid = out["backupId"].as_str().unwrap().to_string();
    assert!(bid.starts_with("startup-"));
    let bdir = ctx.env.data_dir.join("backups").join(&bid);
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(bdir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["kind"], "startup");
    assert!(manifest["createdAt"].as_str().unwrap().contains('T'));
    assert!(manifest["description"].as_str().unwrap().contains("Slack"));
    let rel = manifest["items"][0]["backup"].as_str().unwrap();
    assert_eq!(fs::read_to_string(bdir.join(rel)).unwrap(), SLACK);

    let r = dispatch_json(&ctx, "startup.restore_backup", json!({"id": bid})).unwrap();
    assert_eq!(r["restored"], 1);
    assert_eq!(fs::read_to_string(&f).unwrap(), SLACK);
    // restoring again does not overwrite what is there
    let r = dispatch_json(&ctx, "startup.restore_backup", json!({"id": bid})).unwrap();
    assert_eq!(r["restored"], 0);
}

#[test]
fn remove_aborts_and_deletes_nothing_when_the_backup_cannot_be_written() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, _m) = linux_ctx(d.path());
    let f = autostart(&ctx).join("slack.desktop");
    write(&f, SLACK);
    // `data/backups` is a plain file, so no backup folder can be created.
    fs::create_dir_all(&ctx.env.data_dir).unwrap();
    fs::write(ctx.env.data_dir.join("backups"), "x").unwrap();
    let err = dispatch_json(
        &ctx,
        "startup.remove",
        json!({"id": "xdg:user:slack.desktop"}),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Io);
    assert_eq!(fs::read_to_string(&f).unwrap(), SLACK, "file untouched");
}

#[test]
fn remove_aborts_when_a_staged_backup_step_fails() {
    // The source vanishes between listing and copying: the copy fails and nothing is
    // deleted or left behind.
    let d = tempfile::tempdir().unwrap();
    let (ctx, _m) = linux_ctx(d.path());
    let f = autostart(&ctx).join("slack.desktop");
    write(&f, SLACK);
    let entries = collect(&ctx, &job()).unwrap();
    let e = item(&entries, "xdg:user:slack.desktop").clone();
    fs::remove_file(&f).unwrap();
    assert!(remove_entry(&ctx, &e).is_err());
    let backups = ctx.env.data_dir.join("backups");
    let leftovers = fs::read_dir(&backups).map(|r| r.count()).unwrap_or(0);
    assert_eq!(leftovers, 0, "aborted backup folder is cleaned up");
}

#[test]
fn restore_refuses_backup_ids_and_targets_outside_the_startup_folders() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, _m) = linux_ctx(d.path());
    for bad in ["../x", "startup-1/../../x", "plugins-1", ""] {
        assert!(
            dispatch_json(&ctx, "startup.restore_backup", json!({"id": bad})).is_err(),
            "{bad}"
        );
    }
    // a manifest that points at an arbitrary path
    let bdir = ctx.env.data_dir.join("backups/startup-1700000000");
    write(&bdir.join("files/1-a"), "evil");
    write(
        &bdir.join("manifest.json"),
        &json!({"kind":"startup","items":[{"type":"file","original": d.path().join("evil.txt"),"backup":"files/1-a"}]}).to_string(),
    );
    let err = dispatch_json(
        &ctx,
        "startup.restore_backup",
        json!({"id": "startup-1700000000"}),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(!d.path().join("evil.txt").exists());
}

#[test]
fn removing_a_cron_line_saves_the_crontab_and_restore_appends_the_line() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let sim = Sim::new(&m, Some(CRON));
    let ctx = sim_ctx(d.path(), Os::Linux, &sim);
    let entries = collect(&ctx, &job()).unwrap();
    let id = item(
        &entries,
        &entries
            .iter()
            .find(|e| e.item.command.contains("/opt/x/run"))
            .unwrap()
            .item
            .id,
    )
    .item
    .id
    .clone();
    let out = dispatch_json(&ctx, "startup.remove", json!({"id": id})).unwrap();
    let t = sim.table().unwrap();
    assert!(!t.contains("/opt/x/run"));
    assert!(t.contains("*/5 * * * * /usr/bin/true\n"));
    let bid = out["backupId"].as_str().unwrap();
    let saved = fs::read_to_string(
        ctx.env
            .data_dir
            .join("backups")
            .join(bid)
            .join("files/1-crontab.txt"),
    )
    .unwrap();
    assert_eq!(saved, CRON, "the whole previous crontab is kept");
    dispatch_json(&ctx, "startup.restore_backup", json!({"id": bid})).unwrap();
    assert!(sim.table().unwrap().contains("  @reboot   /opt/x/run   \n"));
}

fn dispatch_json(ctx: &Ctx, method: &str, params: Value) -> crate::error::Result<Value> {
    crate::api::dispatch(ctx, method, params, &job())
}

#[test]
fn list_handler_serialises_the_documented_shape() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, _m) = linux_ctx(d.path());
    write(&autostart(&ctx).join("slack.desktop"), SLACK);
    let v = dispatch_json(&ctx, "startup.list", json!({})).unwrap();
    let a = v.as_array().unwrap();
    assert_eq!(a.len(), 1);
    let o = &a[0];
    for k in [
        "id",
        "name",
        "command",
        "location",
        "kind",
        "scope",
        "enabled",
        "impact",
        "canDisable",
        "canDelete",
        "critical",
    ] {
        assert!(o.get(k).is_some(), "missing {k}: {o}");
    }
    assert_eq!(o["kind"], "autostart");
    assert_eq!(o["scope"], "user");
    assert_eq!(o["impact"], "unknown");
    assert!(o.get("publisher").is_none());
    // wrong ids
    let e = dispatch_json(
        &ctx,
        "startup.set_enabled",
        json!({"id": "xdg:user:nope.desktop", "enabled": false}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    // a client cannot smuggle a path in an id
    let e = dispatch_json(
        &ctx,
        "startup.set_enabled",
        json!({"id": "xdg:user:../../etc/passwd", "enabled": false}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
}

// ---------------------------------------------------------------- impact

#[test]
fn impact_comes_from_running_processes() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let procs = FakeProcesses::with_details(
        vec![
            ProcDetail {
                pid: 10,
                name: "slack".into(),
                exe: Some("/usr/bin/slack".into()),
                memory_bytes: 350 * 1024 * 1024,
                cpu_percent: 1.0,
                is_mine: true,
            },
            ProcDetail {
                pid: 11,
                name: "syncthing".into(),
                exe: Some("/usr/bin/syncthing".into()),
                memory_bytes: 60 * 1024 * 1024,
                cpu_percent: 0.5,
                is_mine: true,
            },
        ],
        true,
    );
    let mut ctx = ctx_os(d.path(), Os::Linux, &m);
    ctx = ctx.with_procs(Arc::new(procs));
    write(&autostart(&ctx).join("slack.desktop"), SLACK);
    write(
        &autostart(&ctx).join("syncthing.desktop"),
        "[Desktop Entry]\nName=Syncthing\nExec=/usr/bin/syncthing serve\n",
    );
    write(
        &autostart(&ctx).join("idle.desktop"),
        "[Desktop Entry]\nName=Idle\nExec=/usr/bin/idle-app\n",
    );
    let entries = collect(&ctx, &job()).unwrap();
    assert_eq!(
        item(&entries, "xdg:user:slack.desktop").item.impact,
        Impact::High
    );
    assert_eq!(
        item(&entries, "xdg:user:syncthing.desktop").item.impact,
        Impact::Medium
    );
    assert_eq!(
        item(&entries, "xdg:user:idle.desktop").item.impact,
        Impact::Unknown
    );
}

// ---------------------------------------------------------------- Windows

#[derive(Default, Clone)]
struct FakeReg {
    values: HashMap<(WinHive, String), Vec<(String, RegData)>>,
    subkeys: HashMap<(WinHive, String), Vec<String>>,
    defaults: HashMap<(WinHive, String), String>,
}

fn k(h: WinHive, key: &str) -> (WinHive, String) {
    (h, key.to_lowercase())
}

impl FakeReg {
    fn value(mut self, h: WinHive, key: &str, name: &str, d: RegData) -> Self {
        self.values
            .entry(k(h, key))
            .or_default()
            .push((name.into(), d));
        self
    }
    fn sub(mut self, h: WinHive, key: &str, names: &[&str]) -> Self {
        self.subkeys
            .entry(k(h, key))
            .or_default()
            .extend(names.iter().map(|s| s.to_string()));
        self
    }
    fn dflt(mut self, h: WinHive, key: &str, v: &str) -> Self {
        self.defaults.insert(k(h, key), v.into());
        self
    }
}

impl StartupRegistry for FakeReg {
    fn values(&self, hive: WinHive, key: &str) -> Vec<(String, RegData)> {
        self.values.get(&k(hive, key)).cloned().unwrap_or_default()
    }
    fn subkeys(&self, hive: WinHive, key: &str) -> Vec<String> {
        self.subkeys.get(&k(hive, key)).cloned().unwrap_or_default()
    }
    fn default_value(&self, hive: WinHive, key: &str) -> Option<String> {
        self.defaults.get(&k(hive, key)).cloned()
    }
    fn key_exists(&self, hive: WinHive, key: &str) -> bool {
        let kk = k(hive, key);
        self.values.contains_key(&kk)
            || self.subkeys.contains_key(&kk)
            || self.defaults.contains_key(&kk)
    }
}

const HKCU_RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const HKLM_RUN: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const HKCU_APPROVED_RUN: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

fn s(v: &str) -> RegData {
    RegData::Str(v.into())
}

fn run_registry() -> FakeReg {
    FakeReg::default()
        .value(
            WinHive::Hkcu,
            HKCU_RUN,
            "Discord",
            s(r#""C:\Users\u\AppData\Local\Discord\Update.exe" --processStart Discord.exe"#),
        )
        .value(
            WinHive::Hkcu,
            HKCU_RUN,
            "Spotify",
            s(r#"C:\Program Files\Spotify\Spotify.exe /minimized"#),
        )
        .value(WinHive::Hkcu, HKCU_RUN, "OneShot", s(r"C:\x\y.exe"))
        .value(
            WinHive::Hkcu,
            HKCU_APPROVED_RUN,
            "Spotify",
            RegData::Binary(vec![3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]),
        )
        .value(
            WinHive::Hkcu,
            HKCU_APPROVED_RUN,
            "Discord",
            RegData::Binary(vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        )
        .value(
            WinHive::Hklm,
            HKLM_RUN,
            "SecurityHealth",
            s(r"C:\Windows\system32\SecurityHealthSystray.exe"),
        )
        .value(
            WinHive::Hklm,
            HKLM_RUN,
            "Dropbox",
            s(r#""C:\Program Files (x86)\Dropbox\Client\Dropbox.exe" /systemstartup"#),
        )
        .value(
            WinHive::Hkcu,
            r"Software\Microsoft\Windows\CurrentVersion\RunOnce",
            "Setup",
            s(r"C:\setup.exe"),
        )
}

fn win_ctx(dir: &Path, m: &MockRunner) -> Ctx {
    ctx_os(dir, Os::Windows, m)
}

#[test]
fn startup_approved_bytes_decide_the_state() {
    assert!(win::approved_enabled(&[2, 0, 0, 0]));
    assert!(win::approved_enabled(&[6, 0, 0, 0]));
    assert!(!win::approved_enabled(&[3, 0, 0, 0, 1, 2]));
    assert!(!win::approved_enabled(&[7]));
    assert!(win::approved_enabled(&[]));
    let en = win::approved_value(true);
    assert_eq!(en, vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let dis = win::approved_value(false);
    assert_eq!(dis.len(), 12);
    assert_eq!(&dis[..4], &[3, 0, 0, 0]);
    let ft = u64::from_le_bytes(dis[4..12].try_into().unwrap());
    // FILETIME of "now": after 2020-01-01
    assert!(ft > 132_223_104_000_000_000, "{ft}");
    assert_eq!(win::hex(&[3, 0, 255]), "0300ff");
}

#[test]
fn windows_run_entries_are_listed_with_state_scope_and_criticality() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let ctx = win_ctx(d.path(), &m);
    let entries = with_registry(run_registry(), || collect(&ctx, &job()).unwrap());
    let discord = item(&entries, "winrun:hkcu-run:Discord");
    assert!(discord.item.enabled && discord.item.can_disable && discord.item.can_delete);
    assert_eq!(discord.item.scope, Scope::User);
    assert_eq!(discord.item.kind, Kind::Autostart);
    assert_eq!(discord.item.location, format!(r"HKCU\{HKCU_RUN}"));
    assert_eq!(
        discord.exe.as_deref(),
        Some(r"C:\Users\u\AppData\Local\Discord\Update.exe")
    );
    let spotify = item(&entries, "winrun:hkcu-run:Spotify");
    assert!(!spotify.item.enabled, "0x03 = disabled");
    assert_eq!(spotify.item.publisher.as_deref(), Some("Spotify"));
    assert!(
        item(&entries, "winrun:hkcu-run:OneShot").item.enabled,
        "no approved value = enabled"
    );
    let sec = item(&entries, "winrun:hklm-run:SecurityHealth");
    assert!(sec.item.critical && !sec.item.can_disable);
    assert_eq!(sec.item.scope, Scope::System);
    let dropbox = item(&entries, "winrun:hklm-run:Dropbox");
    assert!(!dropbox.item.critical);
    assert_eq!(dropbox.item.publisher.as_deref(), Some("Dropbox"));
    let once = item(&entries, "winrun:hkcu-runonce:Setup");
    assert!(!once.item.can_disable && once.item.can_delete);
}

#[test]
fn windows_disable_writes_startup_approved_and_never_deletes_the_run_value() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let ctx = win_ctx(d.path(), &m);
    m.on_any_args(
        "reg",
        CmdOutput::ok("The operation completed successfully."),
    );
    with_registry(run_registry(), || {
        set_enabled_by_id(
            &ctx,
            &job(),
            "winrun:hkcu-run:Discord",
            false,
            ToggleOpts::default(),
        )
        .unwrap();
    });
    let calls: Vec<_> = m.calls().into_iter().filter(|(p, _)| p == "reg").collect();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let (prog, args) = &calls[0];
    assert_eq!(prog, "reg");
    assert_eq!(args[0], "add");
    assert_eq!(args[1], format!(r"HKCU\{HKCU_APPROVED_RUN}"));
    assert_eq!(&args[2..4], &["/v", "Discord"]);
    assert_eq!(&args[4..6], &["/t", "REG_BINARY"]);
    assert_eq!(&args[6], "/d");
    assert!(
        args[7].starts_with("03000000") && args[7].len() == 24,
        "{}",
        args[7]
    );
    assert_eq!(args[8], "/f");
    assert!(!calls.iter().any(|(_, a)| a[0] == "delete"));

    // enabling writes 02 00 00 00...
    let m2 = MockRunner::new();
    let ctx2 = win_ctx(d.path(), &m2);
    m2.on_any_args("reg", CmdOutput::ok(""));
    with_registry(run_registry(), || {
        set_enabled_by_id(
            &ctx2,
            &job(),
            "winrun:hkcu-run:Spotify",
            true,
            ToggleOpts::default(),
        )
        .unwrap();
    });
    let reg_call = m2.calls().into_iter().find(|(p, _)| p == "reg").unwrap();
    assert_eq!(reg_call.1[7], "020000000000000000000000");
}

#[test]
fn windows_machine_wide_entries_use_wow6432_run32_and_elevation() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let ctx = win_ctx(d.path(), &m);
    let reg = FakeReg::default().value(
        WinHive::Hklm,
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run",
        "Old32",
        s(r"C:\Program Files (x86)\Old\old.exe"),
    );
    m.on_any_args("reg", CmdOutput::ok(""));
    with_registry(reg, || {
        with_elevation(true, || {
            set_enabled_by_id(
                &ctx,
                &job(),
                "winrun:hklm-run32:Old32",
                false,
                ToggleOpts::default(),
            )
            .unwrap();
        });
    });
    let args = m.calls().into_iter().find(|(p, _)| p == "reg").unwrap().1;
    assert_eq!(
        args[1],
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run32"
    );
}

#[test]
fn windows_remove_exports_before_deleting_and_aborts_when_export_fails() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let ctx = win_ctx(d.path(), &m);
    // export "succeeds" but writes no file -> treated as a failed backup
    m.on_any_args("reg", CmdOutput::ok(""));
    let r = with_registry(run_registry(), || {
        dispatch_json(
            &ctx,
            "startup.remove",
            json!({"id": "winrun:hkcu-run:Discord"}),
        )
    });
    assert!(r.is_err());
    assert!(
        !m.calls().iter().any(|(_, a)| a[0] == "delete"),
        "nothing deleted: {:?}",
        m.calls()
    );

    // export fails outright
    let m = MockRunner::new();
    let ctx = win_ctx(d.path(), &m);
    m.on_any_args("reg", CmdOutput::failed(1, "ERROR: Access is denied."));
    let r = with_registry(run_registry(), || {
        dispatch_json(
            &ctx,
            "startup.remove",
            json!({"id": "winrun:hkcu-run:Discord"}),
        )
    });
    assert!(r.unwrap_err().message.contains("could not back up"));
    assert!(!m.calls().iter().any(|(_, a)| a[0] == "delete"));
}

/// A `reg` that really writes the export file, so the backup check passes.
struct RegExporter {
    inner: MockRunner,
    log: Arc<Mutex<Vec<Vec<String>>>>,
}

impl CommandRunner for RegExporter {
    fn run(&self, program: &str, args: &[&str]) -> crate::error::Result<CmdOutput> {
        self.log.lock().unwrap().push(
            std::iter::once(program)
                .chain(args.iter().copied())
                .map(str::to_string)
                .collect(),
        );
        if program == "reg" && args.first() == Some(&"export") {
            fs::write(args[2], "Windows Registry Editor Version 5.00\r\n").unwrap();
            return Ok(CmdOutput::ok(""));
        }
        self.inner.run(program, args)
    }
    fn which(&self, p: &str) -> Option<PathBuf> {
        self.inner.which(p)
    }
}

#[test]
fn windows_remove_run_value_order_export_then_delete_and_restore_imports() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    m.on_any_args("reg", CmdOutput::ok(""));
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut ctx = Ctx::new(
        crate::ctx::Env::for_test(d.path()),
        Arc::new(RegExporter {
            inner: m.clone(),
            log: log.clone(),
        }),
    )
    .with_procs(Arc::new(FakeProcesses::new(&[], true)));
    ctx.env.os = Os::Windows;
    let out = with_registry(run_registry(), || {
        dispatch_json(
            &ctx,
            "startup.remove",
            json!({"id": "winrun:hkcu-run:Spotify"}),
        )
        .unwrap()
    });
    let calls = log.lock().unwrap().clone();
    let first_delete = calls
        .iter()
        .position(|c| c[1] == "delete")
        .expect("delete ran");
    let exports: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, c)| c[1] == "export")
        .map(|(i, _)| i)
        .collect();
    assert!(
        !exports.is_empty() && exports.iter().all(|i| *i < first_delete),
        "{calls:?}"
    );
    assert_eq!(calls[exports[0]][2], format!(r"HKCU\{HKCU_RUN}"));
    // the approved key was exported too, because it exists
    assert!(calls
        .iter()
        .any(|c| c[1] == "export" && c[2].ends_with("StartupApproved\\Run")));
    let del = &calls[first_delete];
    assert_eq!(
        &del[2..],
        &[
            format!(r"HKCU\{HKCU_RUN}"),
            "/v".into(),
            "Spotify".into(),
            "/f".into()
        ]
    );

    let bid = out["backupId"].as_str().unwrap();
    log.lock().unwrap().clear();
    dispatch_json(&ctx, "startup.restore_backup", json!({"id": bid})).unwrap();
    let calls = log.lock().unwrap().clone();
    assert_eq!(calls.iter().filter(|c| c[1] == "import").count(), 2);
}

#[test]
fn windows_startup_folder_entries_and_lnk_targets() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let ctx = win_ctx(d.path(), &m);
    let user = win::startup_folders(&ctx)[0].1.clone();
    let all = win::startup_folders(&ctx)[1].1.clone();
    write(&user.join("OneDrive.lnk"), "lnk");
    write(&user.join("run.bat"), "@echo hi");
    write(&user.join("desktop.ini"), "[.ShellClassInfo]");
    write(&all.join("Common.lnk"), "lnk");
    m.on_any_args(
        "powershell",
        CmdOutput::ok(r#"{"Name":"OneDrive.lnk","Target":"C:\\Program Files\\Microsoft OneDrive\\OneDrive.exe","Args":"/background"}"#),
    );
    let reg = FakeReg::default().value(
        WinHive::Hkcu,
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder",
        "run.bat",
        RegData::Binary(vec![3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    );
    let entries = with_registry(reg, || collect(&ctx, &job()).unwrap());
    let od = item(&entries, "winfolder:user:OneDrive.lnk");
    assert_eq!(od.item.name, "OneDrive");
    assert_eq!(
        od.item.command,
        r#""C:\Program Files\Microsoft OneDrive\OneDrive.exe" /background"#
    );
    assert!(od.item.enabled && od.item.can_delete);
    assert!(!item(&entries, "winfolder:user:run.bat").item.enabled);
    assert!(!entries.iter().any(|e| e.item.id.contains("desktop.ini")));
    assert_eq!(
        item(&entries, "winfolder:all:Common.lnk").item.scope,
        Scope::System
    );

    // disabling uses StartupApproved\StartupFolder, not file removal
    let m2 = MockRunner::new();
    m2.on_any_args("reg", CmdOutput::ok(""));
    m2.on_any_args("powershell", CmdOutput::ok(""));
    let ctx2 = win_ctx(d.path(), &m2);
    with_registry(FakeReg::default(), || {
        set_enabled_by_id(
            &ctx2,
            &job(),
            "winfolder:user:OneDrive.lnk",
            false,
            ToggleOpts::default(),
        )
        .unwrap();
    });
    let reg_call = m2.calls().into_iter().find(|(p, _)| p == "reg").unwrap();
    assert!(reg_call.1[1].ends_with(r"StartupApproved\StartupFolder"));
    assert_eq!(reg_call.1[3], "OneDrive.lnk");
    assert!(user.join("OneDrive.lnk").exists());
}

const SCHTASKS: &str = r#""HostName","TaskName","Next Run Time","Status","Logon Mode","Last Run Time","Last Result","Author","Task To Run","Start In","Comment","Scheduled Task State","Idle Time","Power Management","Run As User","Delete Task If Not Rescheduled","Stop Task If Runs X Hours and X Mins","Schedule","Schedule Type","Start Time","Start Date","End Date","Days","Months","Repeat: Every","Repeat: Until: Time","Repeat: Until: Duration","Repeat: Stop If Still Running"
"PC","\GoogleUpdateTaskMachineCore","N/A","Ready","Interactive/Background","1/1/2025","0","Google LLC","C:\Program Files (x86)\Google\Update\GoogleUpdate.exe /c","N/A","Keeps your Google software up to date.
Second line of comment","Enabled","Disabled","Stop On Battery Mode","SYSTEM","Disabled","72:00:00","Scheduling data is not available in this format.","At logon time","N/A","N/A","N/A","N/A","N/A","N/A","N/A","N/A","N/A"
"PC","\GoogleUpdateTaskMachineCore","N/A","Ready","Interactive/Background","1/1/2025","0","Google LLC","C:\Program Files (x86)\Google\Update\GoogleUpdate.exe /c","N/A","","Enabled","Disabled","Stop On Battery Mode","SYSTEM","Disabled","72:00:00","Scheduling data is not available in this format.","One Time Only","N/A","N/A","N/A","N/A","N/A","N/A","N/A","N/A","N/A"
"PC","\MyBackup","N/A","Disabled","Interactive only","N/A","267011","Me","C:\Tools\backup.cmd","N/A","N/A","Disabled","Disabled","Stop On Battery Mode","me","Disabled","72:00:00","Scheduling data is not available in this format.","At logon time","N/A","N/A","N/A","N/A","N/A","N/A","N/A","N/A","N/A"
"HostName","TaskName","Next Run Time","Status","Logon Mode","Last Run Time","Last Result","Author","Task To Run","Start In","Comment","Scheduled Task State","Idle Time","Power Management","Run As User","Delete Task If Not Rescheduled","Stop Task If Runs X Hours and X Mins","Schedule","Schedule Type","Start Time","Start Date","End Date","Days","Months","Repeat: Every","Repeat: Until: Time","Repeat: Until: Duration","Repeat: Stop If Still Running"
"PC","\Microsoft\Windows\Defrag\ScheduledDefrag","N/A","Ready","Interactive/Background","1/1/2025","0","Microsoft Corporation","%windir%\system32\defrag.exe -c -h -o -$","N/A","Comment","Enabled","Disabled","Stop On Battery Mode","SYSTEM","Disabled","72:00:00","Scheduling data is not available in this format.","Weekly","N/A","N/A","N/A","N/A","N/A","N/A","N/A","N/A","N/A"
"#;

#[test]
fn schtasks_csv_with_duplicate_headers_multiline_fields_and_repeated_rows() {
    let tasks = win::parse_schtasks(SCHTASKS);
    let names: Vec<_> = tasks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "\\GoogleUpdateTaskMachineCore",
            "\\MyBackup",
            "\\Microsoft\\Windows\\Defrag\\ScheduledDefrag"
        ]
    );
    assert!(!tasks[0].disabled && tasks[0].run_as == "SYSTEM");
    assert_eq!(tasks[0].author, "Google LLC");
    assert!(tasks[1].disabled, "state text `Disabled`");
    assert_eq!(tasks[1].command, r"C:\Tools\backup.cmd");
    assert!(win::task_is_critical(&tasks[2].name));
    assert!(!win::task_is_critical(&tasks[0].name));
}

#[test]
fn schtasks_localised_output_is_positional() {
    let header = SCHTASKS.lines().next().unwrap();
    let german_header = header
        .replace("HostName", "Hostname")
        .replace("TaskName", "Aufgabenname");
    let text = format!(
        "{german_header}\n\"PC\",\"\\Foo\",\"N/A\",\"Bereit\",\"x\",\"N/A\",\"0\",\"A\",\"C:\\foo.exe\",\"N/A\",\"N/A\",\"Aktiviert\",\"d\",\"p\",\"me\",\"x\",\"x\",\"x\",\"x\"\n\
         {german_header}\n\"PC\",\"\\Bar\",\"N/A\",\"Deaktiviert\",\"x\",\"N/A\",\"0\",\"A\",\"C:\\bar.exe\",\"N/A\",\"N/A\",\"Deaktiviert\",\"d\",\"p\",\"me\",\"x\",\"x\",\"x\",\"x\"\n\
         \"PC\",\"\\Baz\",\"N/A\",\"?\",\"x\",\"N/A\",\"0\",\"A\",\"C:\\baz.exe\",\"N/A\",\"N/A\",\"???\",\"d\",\"p\",\"me\",\"x\",\"x\",\"x\",\"x\"\n"
    );
    let t = win::parse_schtasks(&text);
    assert_eq!(t.len(), 3);
    assert!(!t[0].disabled && !t[0].state_unknown);
    assert!(t[1].disabled);
    assert!(
        !t[2].disabled && t[2].state_unknown,
        "unrecognised language is flagged, not guessed"
    );
}

#[test]
fn csv_parser_handles_quotes_commas_crlf_and_bom() {
    let rows =
        win::parse_csv("\u{feff}\"a\",\"b,c\",\"d\"\"e\"\r\n\"1\",\"\",\"line1\nline2\"\r\n");
    assert_eq!(
        rows,
        vec![vec!["a", "b,c", "d\"e"], vec!["1", "", "line1\nline2"]]
    );
    assert!(win::parse_csv("").is_empty());
}

#[test]
fn windows_tasks_list_toggle_and_delete() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let ctx = win_ctx(d.path(), &m);
    m.on(
        "schtasks",
        &["/query", "/fo", "csv", "/v"],
        CmdOutput::ok(SCHTASKS),
    );
    m.on(
        "schtasks",
        &[
            "/change",
            "/tn",
            "\\GoogleUpdateTaskMachineCore",
            "/disable",
        ],
        CmdOutput::ok("SUCCESS"),
    );
    m.on(
        "schtasks",
        &["/change", "/tn", "\\MyBackup", "/enable"],
        CmdOutput::ok("SUCCESS"),
    );
    let entries = with_registry(FakeReg::default(), || collect(&ctx, &job()).unwrap());
    let g = item(&entries, "wintask:\\GoogleUpdateTaskMachineCore");
    assert_eq!(g.item.kind, Kind::ScheduledTask);
    assert_eq!(g.item.scope, Scope::System);
    assert_eq!(g.item.name, "GoogleUpdateTaskMachineCore");
    assert!(g.item.enabled && g.item.can_disable && !g.item.critical);
    let ms = item(
        &entries,
        "wintask:\\Microsoft\\Windows\\Defrag\\ScheduledDefrag",
    );
    assert!(ms.item.critical && !ms.item.can_disable && !ms.item.can_delete);
    assert_eq!(item(&entries, "wintask:\\MyBackup").item.scope, Scope::User);

    with_registry(FakeReg::default(), || {
        set_enabled_by_id(
            &ctx,
            &job(),
            "wintask:\\GoogleUpdateTaskMachineCore",
            false,
            ToggleOpts::default(),
        )
        .unwrap();
        set_enabled_by_id(
            &ctx,
            &job(),
            "wintask:\\MyBackup",
            true,
            ToggleOpts::default(),
        )
        .unwrap();
        let e = set_enabled_by_id(
            &ctx,
            &job(),
            "wintask:\\Microsoft\\Windows\\Defrag\\ScheduledDefrag",
            false,
            ToggleOpts::default(),
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied);
    });
    assert!(!m
        .calls()
        .iter()
        .any(|(_, a)| a.iter().any(|x| x.contains("Defrag")) && a[0] == "/change"));
}

#[test]
fn windows_task_delete_exports_xml_first_and_restore_recreates() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let ctx = win_ctx(d.path(), &m);
    m.on(
        "schtasks",
        &["/query", "/fo", "csv", "/v"],
        CmdOutput::ok(SCHTASKS),
    );
    m.on(
        "schtasks",
        &["/query", "/tn", "\\MyBackup", "/xml"],
        CmdOutput::ok("<?xml version=\"1.0\"?><Task version=\"1.2\"></Task>"),
    );
    m.on(
        "schtasks",
        &["/delete", "/tn", "\\MyBackup", "/f"],
        CmdOutput::ok("SUCCESS"),
    );
    let out = with_registry(FakeReg::default(), || {
        dispatch_json(&ctx, "startup.remove", json!({"id": "wintask:\\MyBackup"})).unwrap()
    });
    let calls = m.calls();
    let q = calls
        .iter()
        .position(|(_, a)| a.contains(&"/xml".to_string()))
        .unwrap();
    let dl = calls
        .iter()
        .position(|(_, a)| a.contains(&"/delete".to_string()))
        .unwrap();
    assert!(q < dl);
    let bid = out["backupId"].as_str().unwrap();
    let xml = fs::read_to_string(
        ctx.env
            .data_dir
            .join("backups")
            .join(bid)
            .join("files/1-task.xml"),
    )
    .unwrap();
    assert!(xml.contains("<Task"));
    m.on_any_args("schtasks", CmdOutput::ok("SUCCESS"));
    dispatch_json(&ctx, "startup.restore_backup", json!({"id": bid})).unwrap();
    let last = m.calls().pop().unwrap();
    assert_eq!(last.1[0], "/create");
    assert_eq!(last.1[2], "\\MyBackup");
    assert_eq!(last.1[3], "/xml");

    // a failing export aborts the delete
    let m = MockRunner::new();
    let ctx = win_ctx(d.path(), &m);
    m.on(
        "schtasks",
        &["/query", "/fo", "csv", "/v"],
        CmdOutput::ok(SCHTASKS),
    );
    m.on(
        "schtasks",
        &["/query", "/tn", "\\MyBackup", "/xml"],
        CmdOutput::failed(1, "ERROR"),
    );
    let r = with_registry(FakeReg::default(), || {
        dispatch_json(&ctx, "startup.remove", json!({"id": "wintask:\\MyBackup"}))
    });
    assert!(r.is_err());
    assert!(!m
        .calls()
        .iter()
        .any(|(_, a)| a.contains(&"/delete".to_string())));
}

#[test]
fn powershell_service_json_single_object_and_array() {
    let one = r#"{"Name":"Spooler","DisplayName":"Print Spooler","StartMode":"Auto","State":"Running","PathName":"C:\\Windows\\System32\\spoolsv.exe"}"#;
    let v = win::parse_services(one);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].name, "Spooler");
    let many = format!(
        "[{one},{{\"Name\":\"MyApp\",\"DisplayName\":null,\"StartMode\":\"Disabled\",\"State\":\"Stopped\",\"PathName\":null}}]"
    );
    let v = win::parse_services(&many);
    assert_eq!(v.len(), 2);
    assert_eq!(v[1].display, "");
    assert!(win::parse_services("").is_empty());
    assert!(win::parse_services("not json").is_empty());
    assert!(win::parse_services("\u{feff}[]").is_empty());
}

#[test]
fn windows_services_listing_criticality_and_sc_config() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let ctx = win_ctx(d.path(), &m);
    let json = r#"[
      {"Name":"Spooler","DisplayName":"Print Spooler","StartMode":"Auto","State":"Running","PathName":"C:\\Windows\\System32\\spoolsv.exe"},
      {"Name":"WinDefend","DisplayName":"Microsoft Defender Antivirus Service","StartMode":"Auto","State":"Running","PathName":"\"C:\\ProgramData\\Microsoft\\Windows Defender\\Platform\\4.18\\MsMpEng.exe\""},
      {"Name":"AdobeARMservice","DisplayName":"Adobe Acrobat Update Service","StartMode":"Auto","State":"Running","PathName":"\"C:\\Program Files (x86)\\Common Files\\Adobe\\ARM\\1.0\\armsvc.exe\""},
      {"Name":"OldSvc","DisplayName":"Old Service","StartMode":"Disabled","State":"Stopped","PathName":"C:\\Program Files\\Old\\old.exe"},
      {"Name":"Manual1","DisplayName":"Manual","StartMode":"Manual","State":"Stopped","PathName":"C:\\Program Files\\M\\m.exe"}
    ]"#;
    m.on(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            win::SERVICES_SCRIPT,
        ],
        CmdOutput::ok(json),
    );
    m.on(
        "sc.exe",
        &["config", "AdobeARMservice", "start=", "disabled"],
        CmdOutput::ok("[SC] ChangeServiceConfig SUCCESS"),
    );
    let entries = with_registry(FakeReg::default(), || collect(&ctx, &job()).unwrap());
    assert!(!entries.iter().any(|e| e.item.id.ends_with("Manual1")));
    assert!(item(&entries, "winsvc:Spooler").item.critical);
    assert!(item(&entries, "winsvc:WinDefend").item.critical);
    let adobe = item(&entries, "winsvc:AdobeARMservice");
    assert!(!adobe.item.critical && adobe.item.can_disable && adobe.item.enabled);
    assert_eq!(adobe.item.kind, Kind::Service);
    assert!(!item(&entries, "winsvc:OldSvc").item.enabled);
    with_registry(FakeReg::default(), || {
        with_elevation(true, || {
            set_enabled_by_id(
                &ctx,
                &job(),
                "winsvc:AdobeARMservice",
                false,
                ToggleOpts::default(),
            )
            .unwrap();
        });
        let e = set_enabled_by_id(&ctx, &job(), "winsvc:Spooler", false, ToggleOpts::default())
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied);
    });
    assert!(m
        .calls()
        .iter()
        .any(|(p, a)| p == "sc.exe" && a == &["config", "AdobeARMservice", "start=", "disabled"]));
    assert!(!m
        .calls()
        .iter()
        .any(|(_, a)| a.contains(&"Spooler".to_string()) && a[0] == "config"));
    // services cannot be deleted
    let entries = with_registry(FakeReg::default(), || collect(&ctx, &job()).unwrap());
    assert!(remove_entry(&ctx, item(&entries, "winsvc:AdobeARMservice")).is_err());
}

fn ctx_registry() -> FakeReg {
    let base = r"SOFTWARE\Classes";
    let hcm = format!(r"{base}\*\shellex\ContextMenuHandlers");
    FakeReg::default()
        .sub(
            WinHive::Hklm,
            &hcm,
            &["7-Zip", "-Disabled Tool", "Open With", "ShellExt"],
        )
        .dflt(
            WinHive::Hklm,
            &format!(r"{hcm}\7-Zip"),
            "{23170F69-40C1-278A-1000-000100020000}",
        )
        .dflt(
            WinHive::Hklm,
            &format!(r"{hcm}\-Disabled Tool"),
            "{11111111-1111-1111-1111-111111111111}",
        )
        .dflt(
            WinHive::Hklm,
            &format!(r"{hcm}\ShellExt"),
            "{22222222-2222-2222-2222-222222222222}",
        )
        .dflt(
            WinHive::Hklm,
            &format!(r"{base}\CLSID\{{23170F69-40C1-278A-1000-000100020000}}"),
            "7-Zip Shell Extension",
        )
        .dflt(
            WinHive::Hklm,
            &format!(r"{base}\CLSID\{{23170F69-40C1-278A-1000-000100020000}}\InprocServer32"),
            r"C:\Program Files\7-Zip\7-zip.dll",
        )
        .dflt(
            WinHive::Hklm,
            &format!(r"{base}\CLSID\{{11111111-1111-1111-1111-111111111111}}"),
            "Disabled Tool Ext",
        )
        .dflt(
            WinHive::Hklm,
            &format!(r"{base}\CLSID\{{11111111-1111-1111-1111-111111111111}}\InprocServer32"),
            r"C:\Program Files\DisTool\dis.dll",
        )
        // resolves into the Windows directory: a built-in, skipped
        .dflt(
            WinHive::Hklm,
            &format!(r"{base}\CLSID\{{22222222-2222-2222-2222-222222222222}}\InprocServer32"),
            r"%SystemRoot%\system32\shell32.dll",
        )
        .sub(
            WinHive::Hkcu,
            r"Software\Classes\Directory\shellex\ContextMenuHandlers",
            &["UserExt"],
        )
        .dflt(
            WinHive::Hkcu,
            r"Software\Classes\Directory\shellex\ContextMenuHandlers\UserExt",
            "{33333333-3333-3333-3333-333333333333}",
        )
        .dflt(
            WinHive::Hkcu,
            r"Software\Classes\CLSID\{33333333-3333-3333-3333-333333333333}\InprocServer32",
            r"C:\Users\u\AppData\Local\Tool\ext.dll",
        )
}

#[test]
fn context_menu_handlers_are_resolved_and_builtins_skipped() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let ctx = win_ctx(d.path(), &m);
    let entries = with_registry(ctx_registry(), || collect(&ctx, &job()).unwrap());
    let ctxs: Vec<_> = entries
        .iter()
        .filter(|e| e.item.kind == Kind::ContextMenu)
        .collect();
    assert_eq!(
        ctxs.len(),
        3,
        "{:?}",
        ctxs.iter().map(|e| &e.item.id).collect::<Vec<_>>()
    );
    let z = item(&entries, "winctx:hklm:all-files:7-Zip");
    assert_eq!(z.item.name, "7-Zip Shell Extension");
    assert_eq!(z.item.publisher.as_deref(), Some("7-Zip"));
    assert!(z.item.enabled && z.item.can_disable && z.item.can_delete);
    assert_eq!(z.item.scope, Scope::System);
    let dis = item(&entries, "winctx:hklm:all-files:Disabled Tool");
    assert!(!dis.item.enabled, "leading - means disabled");
    assert_eq!(dis.item.name, "Disabled Tool Ext");
    assert_eq!(
        item(&entries, "winctx:hkcu:directory:UserExt").item.scope,
        Scope::User
    );
    assert!(!entries
        .iter()
        .any(|e| e.item.id.contains("Open With") || e.item.id.contains("ShellExt")));
}

#[test]
fn context_menu_disable_exports_then_renames_with_a_dash() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut ctx = Ctx::new(
        crate::ctx::Env::for_test(d.path()),
        Arc::new(RegExporter {
            inner: m.clone(),
            log: log.clone(),
        }),
    )
    .with_procs(Arc::new(FakeProcesses::new(&[], true)));
    ctx.env.os = Os::Windows;
    m.on_any_args("powershell", CmdOutput::ok(""));
    with_registry(ctx_registry(), || {
        with_elevation(true, || {
            set_enabled_by_id(
                &ctx,
                &job(),
                "winctx:hklm:all-files:7-Zip",
                false,
                ToggleOpts::default(),
            )
            .unwrap();
            set_enabled_by_id(
                &ctx,
                &job(),
                "winctx:hklm:all-files:Disabled Tool",
                true,
                ToggleOpts::default(),
            )
            .unwrap();
        });
    });
    let calls = log.lock().unwrap().clone();
    let first_reg = calls.iter().find(|c| c[0] == "reg").unwrap();
    assert_eq!(first_reg[1], "export");
    assert_eq!(
        first_reg[2],
        r"HKLM\SOFTWARE\Classes\*\shellex\ContextMenuHandlers"
    );
    let ps: Vec<String> = calls
        .iter()
        .filter(|c| c[0] == "powershell" && c.contains(&"-EncodedCommand".to_string()))
        .map(|c| powershell_decode(c.last().unwrap()).unwrap())
        .collect();
    assert_eq!(ps.len(), 2);
    assert!(ps[0].contains(r"Rename-Item -LiteralPath 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Classes\*\shellex\ContextMenuHandlers\7-Zip' -NewName '-7-Zip'"), "{}", ps[0]);
    assert!(
        ps[1].contains(r"ContextMenuHandlers\-Disabled Tool' -NewName 'Disabled Tool'"),
        "{}",
        ps[1]
    );
}

#[test]
fn context_menu_names_with_quotes_are_refused_not_injected() {
    let d = tempfile::tempdir().unwrap();
    let m = MockRunner::new();
    let ctx = win_ctx(d.path(), &m);
    let reg = FakeReg::default()
        .sub(
            WinHive::Hkcu,
            r"Software\Classes\*\shellex\ContextMenuHandlers",
            &[r#"evil'; calc; '"x"#],
        )
        .dflt(
            WinHive::Hkcu,
            r#"Software\Classes\*\shellex\ContextMenuHandlers\evil'; calc; '"x"#,
            "{44444444-4444-4444-4444-444444444444}",
        )
        .dflt(
            WinHive::Hkcu,
            r"Software\Classes\CLSID\{44444444-4444-4444-4444-444444444444}\InprocServer32",
            r"C:\Program Files\X\x.dll",
        );
    m.on_any_args("reg", CmdOutput::ok(""));
    m.on_any_args("powershell", CmdOutput::ok(""));
    let r = with_registry(reg, || {
        let entries = collect(&ctx, &job()).unwrap();
        let id = entries
            .iter()
            .find(|e| e.item.kind == Kind::ContextMenu)
            .unwrap()
            .item
            .id
            .clone();
        set_enabled_by_id(&ctx, &job(), &id, false, ToggleOpts::default())
    });
    assert!(r.is_err());
    assert!(!m
        .calls()
        .iter()
        .any(|(p, a)| p == "powershell" && a.contains(&"-EncodedCommand".to_string())));
}

// ---------------------------------------------------------------- macOS

fn write_plist(path: &Path, label: &str, program_args: &[&str], run_at_load: bool, disabled: bool) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut d = plist::Dictionary::new();
    d.insert("Label".into(), plist::Value::String(label.into()));
    d.insert(
        "ProgramArguments".into(),
        plist::Value::Array(
            program_args
                .iter()
                .map(|a| plist::Value::String((*a).into()))
                .collect(),
        ),
    );
    d.insert("RunAtLoad".into(), plist::Value::Boolean(run_at_load));
    if disabled {
        d.insert("Disabled".into(), plist::Value::Boolean(true));
    }
    plist::Value::Dictionary(d).to_file_xml(path).unwrap();
}

const PRINT_DISABLED: &str = "disabled services = {\n\t\"com.zoom.updater\" => disabled\n\t\"com.dropbox.agent\" => enabled\n\t\"com.oldstyle.thing\" => true\n\t\"com.apple.something\" => disabled\n}\n";

fn mac_setup(dir: &Path) -> (Ctx, MockRunner) {
    let m = MockRunner::new();
    let ctx = ctx_os(dir, Os::MacOs, &m);
    m.on("id", &["-u"], CmdOutput::ok("501\n"));
    m.on(
        "launchctl",
        &["print-disabled", "gui/501"],
        CmdOutput::ok(PRINT_DISABLED),
    );
    m.on(
        "launchctl",
        &["print-disabled", "system"],
        CmdOutput::ok("disabled services = {\n}\n"),
    );
    m.on(
        "osascript",
        &["-e", super::macos::LOGIN_ITEMS_SCRIPT],
        CmdOutput::ok("Dropbox\t/Applications/Dropbox.app\nSlack\t/Applications/Slack.app\n"),
    );
    let home = ctx.env.home.join("Library/LaunchAgents");
    write_plist(
        &home.join("com.dropbox.agent.plist"),
        "com.dropbox.agent",
        &[
            "/Applications/Dropbox.app/Contents/MacOS/Dropbox",
            "--agent",
        ],
        true,
        false,
    );
    write_plist(
        &home.join("com.oldstyle.thing.plist"),
        "com.oldstyle.thing",
        &["/usr/local/bin/thing"],
        true,
        false,
    );
    write_plist(
        &ctx.env
            .sys_path("/Library/LaunchAgents")
            .join("com.zoom.updater.plist"),
        "com.zoom.updater",
        &["/Applications/zoom.us.app/Contents/MacOS/updater"],
        true,
        false,
    );
    write_plist(
        &ctx.env
            .sys_path("/Library/LaunchAgents")
            .join("com.apple.something.plist"),
        "com.apple.something",
        &["/usr/bin/something"],
        true,
        false,
    );
    write_plist(
        &ctx.env
            .sys_path("/Library/LaunchDaemons")
            .join("com.vendor.helper.plist"),
        "com.vendor.helper",
        &["/Library/Vendor/helper"],
        true,
        true,
    );
    (ctx, m)
}

#[test]
fn print_disabled_parses_both_spellings() {
    let m = super::macos::parse_print_disabled(PRINT_DISABLED);
    assert!(m["com.zoom.updater"]);
    assert!(!m["com.dropbox.agent"]);
    assert!(m["com.oldstyle.thing"]);
    assert_eq!(m.len(), 4);
    assert!(super::macos::parse_print_disabled("garbage\n{}\n").is_empty());
}

#[test]
fn launchd_items_state_scope_and_apple_items_are_critical() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, _m) = mac_setup(d.path());
    let entries = collect(&ctx, &job()).unwrap();
    let dbx = item(&entries, "mac:home:com.dropbox.agent");
    assert_eq!(dbx.item.kind, Kind::LaunchAgent);
    assert_eq!(dbx.item.scope, Scope::User);
    assert!(dbx.item.enabled && dbx.item.can_disable && dbx.item.can_delete);
    assert_eq!(
        dbx.item.command,
        "/Applications/Dropbox.app/Contents/MacOS/Dropbox --agent"
    );
    assert!(
        !item(&entries, "mac:home:com.oldstyle.thing").item.enabled,
        "=> true means disabled"
    );
    let zoom = item(&entries, "mac:agents:com.zoom.updater");
    assert!(!zoom.item.enabled);
    assert_eq!(zoom.item.scope, Scope::System);
    assert!(!zoom.item.can_delete);
    let apple = item(&entries, "mac:agents:com.apple.something");
    assert!(apple.item.critical && !apple.item.can_disable);
    let helper = item(&entries, "mac:daemons:com.vendor.helper");
    assert_eq!(helper.item.kind, Kind::LaunchDaemon);
    assert!(
        !helper.item.enabled,
        "plist Disabled key without an override"
    );
    let login = item(&entries, "mac:login:Slack");
    assert_eq!(login.item.kind, Kind::LoginItem);
    assert!(!login.item.can_disable && login.item.can_delete);
}

#[test]
fn launchd_disable_uses_launchctl_and_boots_out_only_when_asked() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, m) = mac_setup(d.path());
    m.on_any_args("launchctl", CmdOutput::ok(""));
    // print-disabled outputs are scripted exactly, and exact keys win over on_any_args
    set_enabled_by_id(
        &ctx,
        &job(),
        "mac:home:com.dropbox.agent",
        false,
        ToggleOpts::default(),
    )
    .unwrap();
    let calls = m.calls();
    assert!(calls
        .iter()
        .any(|(p, a)| p == "launchctl" && a == &["disable", "gui/501/com.dropbox.agent"]));
    assert!(!calls
        .iter()
        .any(|(_, a)| a.first().map(String::as_str) == Some("bootout")));
}

#[test]
fn launchd_stop_now_runs_bootout_after_disable() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, m) = mac_setup(d.path());
    m.on_any_args("launchctl", CmdOutput::ok(""));
    set_enabled_by_id(
        &ctx,
        &job(),
        "mac:home:com.dropbox.agent",
        false,
        ToggleOpts { stop_now: true },
    )
    .unwrap();
    let launch: Vec<Vec<String>> = m
        .calls()
        .into_iter()
        .filter(|(p, a)| p == "launchctl" && a[0] != "print-disabled")
        .map(|(_, a)| a)
        .collect();
    assert_eq!(launch[0], vec!["disable", "gui/501/com.dropbox.agent"]);
    assert_eq!(launch[1], vec!["bootout", "gui/501/com.dropbox.agent"]);
    // enabling
    set_enabled_by_id(
        &ctx,
        &job(),
        "mac:agents:com.zoom.updater",
        true,
        ToggleOpts::default(),
    )
    .unwrap();
    assert!(m
        .calls()
        .iter()
        .any(|(_, a)| a == &["enable", "gui/501/com.zoom.updater"]));
}

#[test]
fn launch_daemons_are_changed_elevated_and_apple_items_refused() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, m) = mac_setup(d.path());
    m.on_any_args("launchctl", CmdOutput::ok(""));
    m.on_any_args("osascript", CmdOutput::ok(""));
    let e = set_enabled_by_id(
        &ctx,
        &job(),
        "mac:agents:com.apple.something",
        true,
        ToggleOpts::default(),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    with_elevation(false, || {
        set_enabled_by_id(
            &ctx,
            &job(),
            "mac:daemons:com.vendor.helper",
            true,
            ToggleOpts::default(),
        )
        .unwrap();
    });
    // wrapped in `osascript ... with administrator privileges`
    let scripts: Vec<String> = m
        .calls()
        .into_iter()
        .filter(|(p, a)| {
            p == "osascript"
                && a.get(1)
                    .is_some_and(|s| s.contains("administrator privileges"))
        })
        .map(|(_, a)| a[1].clone())
        .collect();
    assert_eq!(scripts.len(), 1);
    assert!(
        scripts[0].contains("launchctl enable system/com.vendor.helper"),
        "{}",
        scripts[0]
    );
}

#[test]
fn login_item_delete_backs_up_and_uses_system_events() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, m) = mac_setup(d.path());
    m.on_any_args("osascript", CmdOutput::ok(""));
    m.on(
        "osascript",
        &["-e", super::macos::LOGIN_ITEMS_SCRIPT],
        CmdOutput::ok("Slack\t/Applications/Slack.app\n"),
    );
    let out = dispatch_json(&ctx, "startup.remove", json!({"id": "mac:login:Slack"})).unwrap();
    assert!(m.calls().iter().any(|(p, a)| p == "osascript"
        && a[1] == "tell application \"System Events\" to delete login item \"Slack\""));
    dispatch_json(
        &ctx,
        "startup.restore_backup",
        json!({"id": out["backupId"]}),
    )
    .unwrap();
    assert!(m.calls().iter().any(|(p, a)| p == "osascript"
        && a[1].contains("make login item")
        && a[1].contains("/Applications/Slack.app")));
}

#[test]
fn launch_agent_removal_saves_the_plist() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, m) = mac_setup(d.path());
    m.on_any_args("launchctl", CmdOutput::ok(""));
    let plist_path = ctx
        .env
        .home
        .join("Library/LaunchAgents/com.oldstyle.thing.plist");
    let original = fs::read(&plist_path).unwrap();
    let out = dispatch_json(
        &ctx,
        "startup.remove",
        json!({"id": "mac:home:com.oldstyle.thing"}),
    )
    .unwrap();
    assert!(!plist_path.exists());
    dispatch_json(
        &ctx,
        "startup.restore_backup",
        json!({"id": out["backupId"]}),
    )
    .unwrap();
    assert_eq!(fs::read(&plist_path).unwrap(), original);
    // machine-wide agents cannot be deleted
    let e = dispatch_json(
        &ctx,
        "startup.remove",
        json!({"id": "mac:agents:com.zoom.updater"}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
}

#[test]
fn every_platform_lists_something_on_a_bare_machine_without_errors() {
    let d = tempfile::tempdir().unwrap();
    for os in [Os::Linux, Os::Windows, Os::MacOs] {
        let m = MockRunner::new();
        let ctx = ctx_os(d.path(), os, &m);
        assert!(collect(&ctx, &job()).unwrap().is_empty(), "{os:?}");
    }
}

#[test]
fn item_ids_are_unique_and_cancellation_is_honoured() {
    let d = tempfile::tempdir().unwrap();
    let (ctx, _m) = linux_ctx(d.path());
    write(&autostart(&ctx).join("a.desktop"), SLACK);
    write(&sys_autostart(&ctx).join("a.desktop"), SLACK);
    let entries = collect(&ctx, &job()).unwrap();
    assert_eq!(
        entries.len(),
        1,
        "user file overrides the system file of the same name"
    );
    let j = Job::detached();
    j.token().cancel();
    assert_eq!(collect(&ctx, &j).unwrap_err().code, ErrorCode::Cancelled);
}

#[test]
fn needs_admin_matrix() {
    assert!(!needs_admin(&Target::Cron {
        line: String::new(),
        nth: 0
    }));
    assert!(needs_admin(&Target::Systemd {
        unit: "a.service".into(),
        user: false
    }));
    assert!(!needs_admin(&Target::Systemd {
        unit: "a.service".into(),
        user: true
    }));
    assert!(needs_admin(&Target::WinService { name: "x".into() }));
}
