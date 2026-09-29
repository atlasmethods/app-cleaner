//! Running-process access behind a trait so cleaning logic is testable without real apps.

use ::sysinfo as si;
#[cfg(any(test, feature = "testutil"))]
use std::sync::Mutex;

/// A running process, reduced to what the cleaner needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    /// Process name as reported by the OS (`chrome`, `chrome.exe`, `Google Chrome`).
    pub name: String,
}

pub trait ProcessSource: Send + Sync {
    /// Snapshot of running processes.
    fn list(&self) -> Vec<ProcInfo>;
    /// Ask a process to exit gracefully (SIGTERM). Never force-kills.
    /// Returns whether the request could be delivered.
    fn request_exit(&self, pid: u32) -> bool;
}

/// Real process list via `sysinfo`.
///
/// Testing hook: when `CLEARSWEEP_FAKE_PROCESSES` is set (comma separated names, possibly
/// empty) it replaces the real list, so sandboxed end-to-end runs are deterministic even
/// when the developer has a browser open. Nothing is ever signalled in that mode.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemProcesses;

fn fake_from_env() -> Option<Vec<ProcInfo>> {
    let v = std::env::var("CLEARSWEEP_FAKE_PROCESSES").ok()?;
    Some(
        v.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .enumerate()
            .map(|(i, n)| ProcInfo {
                pid: 4_000_000 + i as u32,
                name: n.to_string(),
            })
            .collect(),
    )
}

impl ProcessSource for SystemProcesses {
    fn list(&self) -> Vec<ProcInfo> {
        if let Some(f) = fake_from_env() {
            return f;
        }
        let mut sys = si::System::new();
        sys.refresh_processes(si::ProcessesToUpdate::All, true);
        sys.processes()
            .iter()
            .map(|(pid, p)| ProcInfo {
                pid: pid.as_u32(),
                name: p.name().to_string_lossy().into_owned(),
            })
            .collect()
    }

    fn request_exit(&self, pid: u32) -> bool {
        if fake_from_env().is_some() {
            return false;
        }
        let mut sys = si::System::new();
        let pid = si::Pid::from_u32(pid);
        sys.refresh_processes(si::ProcessesToUpdate::Some(&[pid]), true);
        match sys.process(pid) {
            Some(p) => p.kill_with(si::Signal::Term).unwrap_or(false),
            None => false,
        }
    }
}

/// Does the process `name` match the rule's process name `want`?
/// Case-insensitive; Linux truncates `comm` to 15 bytes, so a long `want` also matches
/// its 15-byte prefix.
pub fn name_matches(name: &str, want: &str) -> bool {
    if name.eq_ignore_ascii_case(want) {
        return true;
    }
    let bytes = want.as_bytes();
    if bytes.len() > 15 {
        if let Some(prefix) = want.get(..15) {
            return name.eq_ignore_ascii_case(prefix);
        }
    }
    false
}

/// Scripted process source for tests. `terminate_closes` decides whether
/// `request_exit` makes the process disappear (a cooperative app).
#[cfg(any(test, feature = "testutil"))]
#[derive(Default)]
pub struct FakeProcesses {
    procs: Mutex<Vec<ProcInfo>>,
    pub terminate_closes: bool,
    exit_requests: Mutex<Vec<u32>>,
}

#[cfg(any(test, feature = "testutil"))]
impl FakeProcesses {
    pub fn new(names: &[&str], terminate_closes: bool) -> Self {
        Self {
            procs: Mutex::new(
                names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| ProcInfo {
                        pid: 1000 + i as u32,
                        name: (*n).to_string(),
                    })
                    .collect(),
            ),
            terminate_closes,
            exit_requests: Mutex::new(Vec::new()),
        }
    }
    pub fn exit_requests(&self) -> Vec<u32> {
        self.exit_requests.lock().unwrap().clone()
    }
}

#[cfg(any(test, feature = "testutil"))]
impl ProcessSource for FakeProcesses {
    fn list(&self) -> Vec<ProcInfo> {
        self.procs.lock().unwrap().clone()
    }
    fn request_exit(&self, pid: u32) -> bool {
        self.exit_requests.lock().unwrap().push(pid);
        if self.terminate_closes {
            self.procs.lock().unwrap().retain(|p| p.pid != pid);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_matching() {
        assert!(name_matches("chrome", "chrome"));
        assert!(name_matches("Chrome.EXE", "chrome.exe"));
        assert!(name_matches("chromium-browse", "chromium-browser"));
        assert!(!name_matches("chromium", "chrome"));
        assert!(!name_matches("notchrome", "chrome"));
    }

    #[test]
    fn system_source_lists_this_process() {
        let me = std::process::id();
        assert!(SystemProcesses.list().iter().any(|p| p.pid == me));
    }
}
