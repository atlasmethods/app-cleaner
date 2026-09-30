//! App-level helpers for the About page.
//!
//! Methods:
//! - `system.app_info`: `{name, version, license, dataDir, os}`.
//! - `system.open_data_dir`: show ClearSweep's data folder (settings, history, backups) in the
//!   file manager (`xdg-open` / `open` / `explorer`). Returns `{path}` at once; the file
//!   manager is started in the background and its failure is not reported.

use serde_json::{json, Value};

use crate::api::Registry;
use crate::ctx::{Ctx, Os};
use crate::error::{ApiError, Result};
use crate::job::Job;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &["system.app_info", "system.open_data_dir"];

pub fn register(r: &mut Registry) {
    r.add("system.app_info", app_info);
    r.add("system.open_data_dir", open_data_dir);
}

fn app_info(ctx: &Ctx, _p: Value, _job: &Job) -> Result<Value> {
    Ok(json!({
        "name": "ClearSweep",
        "version": env!("CARGO_PKG_VERSION"),
        "license": env!("CARGO_PKG_LICENSE"),
        "dataDir": ctx.env.data_dir.to_string_lossy(),
        "os": match ctx.env.os { Os::Linux => "linux", Os::Windows => "windows", Os::MacOs => "macos" },
    }))
}

/// The file manager launcher for `os`.
pub fn opener(os: Os) -> &'static str {
    match os {
        Os::Linux => "xdg-open",
        Os::Windows => "explorer",
        Os::MacOs => "open",
    }
}

fn open_data_dir(ctx: &Ctx, _p: Value, _job: &Job) -> Result<Value> {
    let dir = &ctx.env.data_dir;
    std::fs::create_dir_all(dir)?;
    let program = opener(ctx.env.os);
    if ctx.runner.which(program).is_none() {
        return Err(ApiError::unsupported(format!(
            "`{program}` was not found; open {} yourself",
            dir.display()
        )));
    }
    let path = dir.to_string_lossy().into_owned();
    let runner = ctx.runner.clone();
    // File managers may stay in the foreground (and explorer exits with 1 on success).
    std::thread::spawn(move || {
        let _ = runner.run(program, &[&path]);
    });
    Ok(json!({ "path": dir.to_string_lossy() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dispatch;
    use crate::runner::{CmdOutput, MockRunner};

    #[test]
    fn app_info_reports_the_build() {
        let d = tempfile::tempdir().unwrap();
        let c = Ctx::test(d.path(), MockRunner::new());
        let v = dispatch(&c, "system.app_info", Value::Null, &Job::detached()).unwrap();
        assert_eq!(v["name"], "ClearSweep");
        assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["license"], "MIT");
        assert_eq!(v["dataDir"], c.env.data_dir.to_string_lossy().as_ref());
    }

    #[test]
    fn open_data_dir_launches_the_file_manager_per_os() {
        for (os, prog) in [
            (Os::Linux, "xdg-open"),
            (Os::Windows, "explorer"),
            (Os::MacOs, "open"),
        ] {
            let d = tempfile::tempdir().unwrap();
            let m = MockRunner::new();
            m.on_any_args(prog, CmdOutput::ok(""));
            let mut c = Ctx::test(d.path(), m.clone());
            c.env.os = os;
            let v = dispatch(&c, "system.open_data_dir", Value::Null, &Job::detached()).unwrap();
            assert_eq!(v["path"], c.env.data_dir.to_string_lossy().as_ref());
            assert!(
                c.env.data_dir.is_dir(),
                "the folder is created so there is something to open"
            );
            // The launch is asynchronous.
            for _ in 0..100 {
                if !m.calls().is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let calls = m.calls();
            assert_eq!(calls.len(), 1, "{os:?}");
            assert_eq!(calls[0].0, prog);
            assert_eq!(calls[0].1, [c.env.data_dir.to_string_lossy().to_string()]);
        }
    }

    #[test]
    fn a_missing_file_manager_is_unsupported() {
        let d = tempfile::tempdir().unwrap();
        let c = Ctx::test(d.path(), MockRunner::new());
        let e = dispatch(&c, "system.open_data_dir", Value::Null, &Job::detached()).unwrap_err();
        assert_eq!(e.code, crate::error::ErrorCode::Unsupported);
    }
}
