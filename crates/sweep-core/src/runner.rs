//! Abstraction over spawning external commands so features are unit-testable.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

use crate::error::{ApiError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    /// Exit code (-1 if terminated by a signal).
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            status: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }
    pub fn failed(status: i32, stderr: impl Into<String>) -> Self {
        Self {
            status,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
    pub fn success(&self) -> bool {
        self.status == 0
    }
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput>;
    fn which(&self, program: &str) -> Option<PathBuf>;
}

/// Runs real processes via `std::process`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput> {
        let out = Command::new(program).args(args).output().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ApiError::not_found(format!("command not found: {program}"))
            } else {
                ApiError::io(format!("failed to run {program}: {e}"))
            }
        })?;
        Ok(CmdOutput {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    fn which(&self, program: &str) -> Option<PathBuf> {
        let p = std::path::Path::new(program);
        if p.components().count() > 1 {
            return p.is_file().then(|| p.to_path_buf());
        }
        let path = std::env::var_os("PATH")?;
        let exts: Vec<String> = if cfg!(windows) {
            std::env::var("PATHEXT")
                .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
                .split(';')
                .map(str::to_string)
                .collect()
        } else {
            vec![String::new()]
        };
        for dir in std::env::split_paths(&path) {
            for ext in &exts {
                let cand = dir.join(format!("{program}{ext}"));
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
        None
    }
}

#[derive(Default)]
struct MockState {
    calls: Vec<(String, Vec<String>)>,
    outputs: HashMap<String, std::result::Result<CmdOutput, ApiError>>,
    which: HashMap<String, PathBuf>,
}

/// Test double: records calls and returns scripted outputs keyed by program + args.
///
/// Clones share state, so a test can keep a clone to inspect calls after handing the
/// runner to a `Ctx`.
#[derive(Clone, Default)]
pub struct MockRunner {
    state: Arc<Mutex<MockState>>,
}

impl MockRunner {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(program: &str, args: &[&str]) -> String {
        std::iter::once(program)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Script the output for `program args...`. Also makes `which(program)` succeed.
    pub fn on(&self, program: &str, args: &[&str], output: CmdOutput) -> &Self {
        let mut s = self.state.lock().unwrap();
        s.outputs.insert(Self::key(program, args), Ok(output));
        s.which
            .entry(program.to_string())
            .or_insert_with(|| PathBuf::from(format!("/mock/bin/{program}")));
        self
    }

    /// Script an error for `program args...`.
    pub fn on_err(&self, program: &str, args: &[&str], err: ApiError) -> &Self {
        self.state
            .lock()
            .unwrap()
            .outputs
            .insert(Self::key(program, args), Err(err));
        self
    }

    /// Declare that `program` exists on PATH.
    pub fn with_program(&self, program: &str) -> &Self {
        self.state.lock().unwrap().which.insert(
            program.to_string(),
            PathBuf::from(format!("/mock/bin/{program}")),
        );
        self
    }

    /// All recorded calls as `(program, args)`.
    pub fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.state.lock().unwrap().calls.clone()
    }
}

impl CommandRunner for MockRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput> {
        let mut s = self.state.lock().unwrap();
        s.calls.push((
            program.to_string(),
            args.iter().map(|a| a.to_string()).collect(),
        ));
        match s.outputs.get(&Self::key(program, args)) {
            Some(r) => r.clone(),
            None => Err(ApiError::not_found(format!(
                "MockRunner: no scripted output for `{}`",
                Self::key(program, args)
            ))),
        }
    }

    fn which(&self, program: &str) -> Option<PathBuf> {
        self.state.lock().unwrap().which.get(program).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    #[test]
    fn mock_records_calls_and_returns_scripted_output() {
        let m = MockRunner::new();
        m.on("apt", &["list"], CmdOutput::ok("hello"));
        let out = m.run("apt", &["list"]).unwrap();
        assert_eq!(out.stdout, "hello");
        assert!(out.success());
        assert_eq!(
            m.calls(),
            vec![("apt".to_string(), vec!["list".to_string()])]
        );
        assert!(m.which("apt").is_some());
        assert!(m.which("dnf").is_none());
    }

    #[test]
    fn mock_unscripted_is_not_found_and_still_recorded() {
        let m = MockRunner::new();
        let err = m.run("nope", &["x"]).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        assert_eq!(m.calls().len(), 1);
    }

    #[test]
    fn mock_clone_shares_state() {
        let m = MockRunner::new();
        let m2 = m.clone();
        m2.on_err("x", &[], ApiError::permission_denied("no"));
        assert_eq!(
            m.run("x", &[]).unwrap_err().code,
            ErrorCode::PermissionDenied
        );
        assert_eq!(m2.calls().len(), 1);
    }

    #[test]
    fn system_runner_missing_program_is_not_found() {
        let err = SystemRunner
            .run("definitely-not-a-real-program-xyz", &[])
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(SystemRunner
            .which("definitely-not-a-real-program-xyz")
            .is_none());
    }
}
