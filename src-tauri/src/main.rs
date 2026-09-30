// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

/// The desktop executable doubles as the headless command line: OS schedulers and autostart
/// entries launch `<exe> clean --auto ...` / `<exe> agent`, which must never open a window.
fn main() -> ExitCode {
    let first = std::env::args_os().nth(1);
    if first
        .as_deref()
        .and_then(|a| a.to_str())
        .is_some_and(sweep_cli::is_headless_command)
    {
        return sweep_cli::run(std::env::args_os());
    }
    clearsweep_desktop_lib::run()
}
