//! `clearsweep` command line interface.

use clap::{Parser, Subcommand};
use serde_json::Value;
use std::io::Write;
use std::process::ExitCode;
use sweep_core::features::cleaner::{self, CleanOptions, CleanReport, Source};
use sweep_core::features::settings;
use sweep_core::{dispatch, ApiError, Ctx, Job};
use sweep_server::{generate_token, serve, ServeOptions};

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

fn cmd_clean(auto: bool, rules: Option<Vec<String>>, source: SourceArg, json: bool) -> ExitCode {
    if !auto {
        print_error(&ApiError::invalid_params(
            "interactive cleaning is done in the app; pass --auto to clean with the saved settings",
        ));
        return ExitCode::from(2);
    }
    let ctx = Ctx::system();
    let s = settings::load(&ctx);
    // Nobody can answer "ask" here, so it behaves like "skip".
    let opts = CleanOptions::from_settings(&s, true);
    let source = match source {
        SourceArg::Auto => Source::Auto,
        SourceArg::Scheduled => Source::Scheduled,
    };
    match cleaner::run_clean(&ctx, rules, &opts, source, &Job::detached()) {
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

fn main() -> ExitCode {
    let cli = Cli::parse();
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
            json,
        } => cmd_clean(auto, rules, source, json),
        Command::Analyze { rules, json } => cmd_analyze(rules, json),
        #[cfg(feature = "testutil")]
        Command::DevFixture { dir } => cmd_dev_fixture(&dir),
    }
}
