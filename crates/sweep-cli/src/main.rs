//! `clearsweep` command line interface.

use clap::{Parser, Subcommand};
use serde_json::Value;
use std::io::Write;
use std::process::ExitCode;
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
    /// Run a clean (not implemented yet)
    Clean {
        /// Run with the saved settings without prompting
        #[arg(long)]
        auto: bool,
    },
    /// Analyze without deleting (not implemented yet)
    Analyze,
}

fn print_error(e: &ApiError) {
    eprintln!(
        "{}",
        serde_json::to_string(e).unwrap_or_else(|_| e.to_string())
    );
}

fn not_implemented(what: &str) -> ExitCode {
    print_error(&ApiError::not_implemented(format!(
        "`{what}` is not implemented yet"
    )));
    ExitCode::from(2)
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
        Command::Clean { auto } => not_implemented(if auto { "clean --auto" } else { "clean" }),
        Command::Analyze => not_implemented("analyze"),
    }
}
