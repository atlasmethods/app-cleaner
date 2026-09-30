//! The executable that OS schedulers and autostart entries should launch.

use std::path::PathBuf;

/// Environment variable that overrides [`job_exe`] (tests, portable installs).
pub const EXE_ENV: &str = "CLEARSWEEP_EXE";

/// The program to put into OS schedulers / autostart entries: `CLEARSWEEP_EXE` when set,
/// otherwise the running executable. The desktop binary understands the same headless
/// subcommands as the CLI (`agent`, `clean`, ...), so either can be used.
pub fn job_exe() -> PathBuf {
    if let Some(p) = std::env::var_os(EXE_ENV).filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("clearsweep"))
}
