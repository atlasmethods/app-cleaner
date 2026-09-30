//! `clearsweep` command line interface, as a library so the desktop executable can run the
//! same headless commands (`clean`, `analyze`, `agent`, `call`): OS schedulers and autostart
//! entries may launch either binary.

use clap::{Parser, Subcommand};
use serde_json::Value;
use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use sweep_core::agent::{self, Agent, AgentConfig, AgentLock, Event};
use sweep_core::clock::SystemClock;
use sweep_core::features::cleaner::{self, CleanOptions, CleanReport, Source};
use sweep_core::features::{scheduler, settings};
use sweep_core::{dispatch, ApiError, Ctx, Job};
use sweep_server::{generate_token, serve, ServeOptions};

pub mod notify;

/// Subcommands that never open a window; the desktop executable hands these to [`run`].
pub const HEADLESS_COMMANDS: &[&str] = &["clean", "analyze", "agent", "call"];

pub fn is_headless_command(arg: &str) -> bool {
    HEADLESS_COMMANDS.contains(&arg)
}

#[derive(Parser)]
#[command(name = "clearsweep", version, about = "ClearSweep system cleaner")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the local web UI and open it in the default browser
    Ui {
        /// Port to listen on (0 = random free port)
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Do not open a browser window
        #[arg(long)]
        no_open: bool,
        /// Keep running even when the browser tab is closed
        #[arg(long)]
        no_exit_on_idle: bool,
        /// Also print the bare URL on its own line (machine readable)
        #[arg(long)]
        print_url: bool,
    },
    /// Call an API method in-process and print the JSON result
    Call {
        /// Method name, e.g. `sysinfo.get`
        method: String,
        /// JSON parameters (default: null)
        params: Option<String>,
    },
    /// Clean the selected rules (only `--auto` is available on the command line)
    Clean {
        /// Run with the saved settings without prompting
        #[arg(long)]
        auto: bool,
        /// Comma separated rule ids (default: the rules enabled in Settings)
        #[arg(long, value_delimiter = ',')]
        rules: Option<Vec<String>>,
        /// Recorded in the cleaning history
        #[arg(long, value_enum, default_value_t = SourceArg::Auto)]
        source: SourceArg,
        /// Run scheduled clean `<id>` (its rules; recorded as `scheduled`, and its last run
        /// is stored). This is what the OS scheduler launches. Cannot be combined with --rules.
        #[arg(long, conflicts_with = "rules")]
        schedule: Option<String>,
        /// Print the full report as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show what a clean would remove, without deleting anything
    Analyze {
        /// Comma separated rule ids (default: the rules enabled in Settings)
        #[arg(long, value_delimiter = ',')]
        rules: Option<Vec<String>>,
        /// Print the full report as JSON
        #[arg(long)]
        json: bool,
    },
    /// Run the background agent: smart cleaning, sleep-mode enforcement and missed scheduled
    /// cleans. Exits at once when another agent is already running.
    Agent {
        /// Run every task once and exit (for tests and troubleshooting)
        #[arg(long)]
        once: bool,
        /// With --once: how many times to poll the browsers back to back, so that a close
        /// can be observed (a browser must be seen closed twice in a row)
        #[arg(long, default_value_t = 3, hide = true)]
        polls: u32,
    },
    /// Build a fake machine under DIR for end-to-end tests (test builds only)
    #[cfg(feature = "testutil")]
    #[command(hide = true)]
    DevFixture {
        /// A directory named `clearsweep-e2e-*` or `clearsweep-test-*`
        dir: std::path::PathBuf,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum SourceArg {
    Auto,
    Scheduled,
}

fn print_error(e: &ApiError) {
    eprintln!(
        "{}",
        serde_json::to_string(e).unwrap_or_else(|_| e.to_string())
    );
}

fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

fn cmd_analyze(rules: Option<Vec<String>>, json: bool) -> ExitCode {
    let ctx = Ctx::system();
    match cleaner::analyze(&ctx, rules, &Job::detached()) {
        Err(e) => {
            print_error(&e);
            ExitCode::from(1)
        }
        Ok(report) if json => {
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            ExitCode::SUCCESS
        }
        Ok(report) => {
            let mut shown = 0;
            println!("{:<44} {:>10} {:>8} {:>8}", "Rule", "Size", "Files", "Rows");
            for it in &report.items {
                let something = it.files > 0 || it.rows > 0 || !it.actions.is_empty();
                if !something && it.errors.is_empty() {
                    continue;
                }
                shown += 1;
                let name = format!("{} - {}", it.group, it.name);
                let mut note = String::new();
                if it.app_running {
                    note.push_str("  [app running]");
                }
                if !it.actions.is_empty() {
                    note.push_str(&format!("  [{}]", it.actions.join(", ")));
                }
                println!(
                    "{:<44} {:>10} {:>8} {:>8}{}",
                    name,
                    human_bytes(it.bytes),
                    it.files,
                    it.rows,
                    note
                );
                for e in &it.errors {
                    println!("    ! {}: {}", e.path, e.message);
                }
            }
            if shown == 0 {
                println!("(nothing to clean)");
            }
            println!(
                "\nTotal: {} in {} files, {} database rows ({} ms)",
                human_bytes(report.total_bytes),
                report.total_files,
                report.total_rows,
                report.duration_ms
            );
            ExitCode::SUCCESS
        }
    }
}

fn print_clean_summary(r: &CleanReport) {
    for res in &r.results {
        let what = match res.skipped {
            Some(sweep_core::features::cleaner::Skipped::AppRunning) => {
                format!("skipped (app running: {})", res.running_apps.join(", "))
            }
            Some(sweep_core::features::cleaner::Skipped::InUse) => "skipped (in use)".to_string(),
            Some(sweep_core::features::cleaner::Skipped::Unsupported) => {
                "skipped (not supported here)".to_string()
            }
            None => format!(
                "removed {} in {} files, {} rows{}",
                human_bytes(res.removed_bytes),
                res.removed_files,
                res.removed_rows,
                if res.failed.is_empty() {
                    String::new()
                } else {
                    format!(", {} failed", res.failed.len())
                }
            ),
        };
        println!("{:<28} {}", res.rule_id, what);
    }
    println!(
        "\nTotal: removed {} in {} files, {} database rows ({} ms){}",
        human_bytes(r.total_bytes),
        r.total_files,
        r.total_rows,
        r.duration_ms,
        if r.cancelled { " - cancelled" } else { "" }
    );
}

fn cmd_clean(
    auto: bool,
    rules: Option<Vec<String>>,
    source: SourceArg,
    schedule: Option<String>,
    json: bool,
) -> ExitCode {
    if !auto {
        print_error(&ApiError::invalid_params(
            "interactive cleaning is done in the app; pass --auto to clean with the saved settings",
        ));
        return ExitCode::from(2);
    }
    let ctx = Ctx::system();
    let report_result = match schedule {
        Some(id) => match run_scheduled(&ctx, &id) {
            Ok(Some(r)) => Ok(r),
            Ok(None) => return ExitCode::SUCCESS,
            Err(e) => Err(e),
        },
        None => {
            let s = settings::load(&ctx);
            // Nobody can answer "ask" here, so it behaves like "skip".
            let opts = CleanOptions::from_settings(&s, true);
            let source = match source {
                SourceArg::Auto => Source::Auto,
                SourceArg::Scheduled => Source::Scheduled,
            };
            cleaner::run_clean(&ctx, rules, &opts, source, &Job::detached())
        }
    };
    match report_result {
        Err(e) => {
            print_error(&e);
            ExitCode::from(1)
        }
        Ok(report) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            } else {
                print_clean_summary(&report);
            }
            ExitCode::SUCCESS
        }
    }
}

/// Run scheduled clean `id` and record `lastRun` / `lastResult`. `Ok(None)` = the schedule is
/// disabled and nothing was done (an OS job that outlived a disable must stay harmless).
fn run_scheduled(ctx: &Ctx, id: &str) -> Result<Option<CleanReport>, ApiError> {
    let all = scheduler::load(ctx)?;
    let Some(s) = all.iter().find(|s| s.id == id) else {
        return Err(ApiError::not_found(format!(
            "no schedule with id `{id}` (it was removed; its OS job can be deleted)"
        )));
    };
    if !s.enabled {
        eprintln!("schedule `{}` is disabled; nothing to do", s.name);
        return Ok(None);
    }
    let outcome = scheduler::run_schedule(ctx, id, &Job::detached(), true)?;
    match (outcome.error, outcome.report) {
        (Some(e), _) => Err(e),
        (None, Some(r)) => Ok(Some(r)),
        (None, None) => Err(ApiError::internal("the clean produced no report")),
    }
}

fn describe_event(e: &Event) -> String {
    match e {
        Event::JunkChecked {
            bytes,
            over_threshold,
        } => format!(
            "junk check: {}{}",
            human_bytes(*bytes),
            if *over_threshold {
                " (over the threshold)"
            } else {
                ""
            }
        ),
        Event::Notified { body, .. } => format!("notification: {body}"),
        Event::AutoCleaned { bytes } => format!("cleaned {} automatically", human_bytes(*bytes)),
        Event::BrowserCleaned { group, bytes } => {
            format!("cleaned {} after {group} closed", human_bytes(*bytes))
        }
        Event::Enforced { changed } => {
            format!("sleep mode: {changed} startup item(s) kept disabled")
        }
        Event::ScheduleCaughtUp { id } => format!("ran missed scheduled clean {id}"),
        Event::Error(m) => format!("error: {m}"),
    }
}

fn cmd_agent(once: bool, polls: u32) -> ExitCode {
    let ctx = Ctx::system();
    let lock = match AgentLock::try_acquire(&ctx) {
        Ok(Some(l)) => l,
        Ok(None) => {
            eprintln!("another ClearSweep agent is already running; nothing to do");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            print_error(&e);
            return ExitCode::from(1);
        }
    };
    let mut agent = Agent::with_config(
        ctx,
        Arc::new(SystemClock),
        Arc::new(notify::DesktopNotifier),
        AgentConfig::default(),
    );
    if once {
        for e in agent.run_once(polls) {
            println!("{}", describe_event(&e));
        }
        agent.shutdown();
        drop(lock);
        return ExitCode::SUCCESS;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let cancel = agent.cancel_token();
    let handler_stop = stop.clone();
    // SIGINT, SIGTERM and SIGHUP (Ctrl-C / Ctrl-Break / console close on Windows).
    if let Err(e) = ctrlc::set_handler(move || {
        handler_stop.store(true, Ordering::SeqCst);
        cancel.cancel();
    }) {
        eprintln!("could not install the shutdown handler: {e}");
        return ExitCode::from(1);
    }
    eprintln!("ClearSweep agent started (pid {})", std::process::id());
    agent::run_loop(&mut agent, &stop, |events| {
        for e in events {
            eprintln!("{}", describe_event(e));
        }
    });
    eprintln!("ClearSweep agent stopped");
    drop(lock);
    ExitCode::SUCCESS
}

#[cfg(feature = "testutil")]
fn cmd_dev_fixture(dir: &std::path::Path) -> ExitCode {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !(name.starts_with("clearsweep-e2e-") || name.starts_with("clearsweep-test-")) {
        eprintln!("refusing: directory name must start with clearsweep-e2e- or clearsweep-test-");
        return ExitCode::from(2);
    }
    for sub in ["home", "root", "data"] {
        let p = dir.join(sub);
        if p.exists() {
            if let Err(e) = std::fs::remove_dir_all(&p) {
                eprintln!("cannot reset {}: {e}", p.display());
                return ExitCode::from(1);
            }
        }
    }
    let fx = sweep_core::testutil::Fixture::new(dir);
    fx.populate_typical();
    ExitCode::SUCCESS
}

fn cmd_call(method: &str, params: Option<&str>) -> ExitCode {
    let params: Value = match params {
        None => Value::Null,
        Some(s) => match serde_json::from_str(s) {
            Ok(v) => v,
            Err(e) => {
                print_error(&ApiError::invalid_params(format!(
                    "params is not valid JSON: {e}"
                )));
                return ExitCode::from(1);
            }
        },
    };
    let ctx = Ctx::system();
    match dispatch(&ctx, method, params, &Job::detached()) {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
            ExitCode::SUCCESS
        }
        Err(e) => {
            print_error(&e);
            ExitCode::from(1)
        }
    }
}

fn cmd_ui(port: u16, no_open: bool, no_exit_on_idle: bool, print_url: bool) -> ExitCode {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to start async runtime: {e}");
            return ExitCode::from(1);
        }
    };
    rt.block_on(async move {
        let opts = ServeOptions {
            port,
            token: generate_token(),
            exit_on_idle: !no_exit_on_idle,
            ..ServeOptions::default()
        };
        let (url, handle) = match serve(opts).await {
            Ok(x) => x,
            Err(e) => {
                eprintln!("failed to start server: {e}");
                return ExitCode::from(1);
            }
        };
        {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "ClearSweep running at {url}");
            if print_url {
                let _ = writeln!(out, "{url}");
            }
            let _ = out.flush();
        }
        if !no_open {
            if let Err(e) = webbrowser::open(&url) {
                eprintln!("could not open a browser ({e}); open the URL above manually");
            }
        }
        tokio::select! {
            _ = handle.wait() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
        ExitCode::SUCCESS
    })
}

/// Run the command line with `args` (the first is the program name).
pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let cli = Cli::parse_from(args);
    match cli.command {
        Command::Ui {
            port,
            no_open,
            no_exit_on_idle,
            print_url,
        } => cmd_ui(port, no_open, no_exit_on_idle, print_url),
        Command::Call { method, params } => cmd_call(&method, params.as_deref()),
        Command::Clean {
            auto,
            rules,
            source,
            schedule,
            json,
        } => cmd_clean(auto, rules, source, schedule, json),
        Command::Analyze { rules, json } => cmd_analyze(rules, json),
        Command::Agent { once, polls } => cmd_agent(once, polls),
        #[cfg(feature = "testutil")]
        Command::DevFixture { dir } => cmd_dev_fixture(&dir),
    }
}
