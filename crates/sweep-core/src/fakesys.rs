//! A scripted "operating system" for tests: `systemctl`, `crontab`, `schtasks`, `reg`,
//! `launchctl` and `id` with just enough state to check what ClearSweep installed.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::error::{ApiError, Result};
use crate::runner::{CmdOutput, CommandRunner};

#[derive(Default)]
pub struct SysState {
    pub calls: Vec<Vec<String>>,
    /// `None` = no crontab for the user.
    pub crontab: Option<String>,
    pub systemd_user: bool,
    pub present: HashSet<String>,
    pub uid: Option<String>,
    /// `(command prefix, status, stderr)`: matching commands fail like that.
    pub fail: Vec<(String, i32, String)>,
}

#[derive(Clone, Default)]
pub struct FakeSys {
    pub state: Arc<Mutex<SysState>>,
}

impl FakeSys {
    pub fn new(programs: &[&str]) -> Self {
        let s = FakeSys::default();
        {
            let mut st = s.state.lock().unwrap();
            st.present = programs.iter().map(|p| p.to_string()).collect();
            st.systemd_user = programs.contains(&"systemctl");
            st.uid = Some("501".into());
        }
        s
    }
    pub fn fail(&self, prefix: &str, status: i32, stderr: &str) {
        self.state
            .lock()
            .unwrap()
            .fail
            .push((prefix.to_string(), status, stderr.to_string()));
    }
    pub fn set_crontab(&self, text: Option<&str>) {
        self.state.lock().unwrap().crontab = text.map(String::from);
    }
    pub fn crontab(&self) -> Option<String> {
        self.state.lock().unwrap().crontab.clone()
    }
    /// Every command run so far, as `program arg arg`.
    pub fn calls(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .map(|c| c.join(" "))
            .collect()
    }
    /// Calls to `program` only, without the program name.
    pub fn calls_of(&self, program: &str) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c[0] == program)
            .map(|c| c[1..].join(" "))
            .collect()
    }
    pub fn clear_calls(&self) {
        self.state.lock().unwrap().calls.clear();
    }
}

impl CommandRunner for FakeSys {
    fn run(&self, program: &str, args: &[&str]) -> Result<CmdOutput> {
        let mut st = self.state.lock().unwrap();
        let mut call = vec![program.to_string()];
        call.extend(args.iter().map(|a| a.to_string()));
        let line = call.join(" ");
        st.calls.push(call);
        if !st.present.contains(program) {
            return Err(ApiError::not_found(format!("command not found: {program}")));
        }
        if let Some((_, status, err)) = st.fail.iter().find(|(p, _, _)| line.starts_with(p)) {
            return Ok(CmdOutput::failed(*status, err.clone()));
        }
        match (program, args) {
            ("systemctl", ["--user", "show-environment"]) => Ok(if st.systemd_user {
                CmdOutput::ok("PATH=/usr/bin\n")
            } else {
                CmdOutput::failed(1, "Failed to connect to bus")
            }),
            ("crontab", ["-l"]) => Ok(match &st.crontab {
                Some(t) => CmdOutput::ok(t.clone()),
                None => CmdOutput::failed(1, "no crontab for user"),
            }),
            ("crontab", [file]) => {
                let text = std::fs::read_to_string(PathBuf::from(file))
                    .map_err(|e| ApiError::io(format!("fake crontab cannot read {file}: {e}")))?;
                st.crontab = Some(text);
                Ok(CmdOutput::ok(""))
            }
            ("id", ["-u"]) => Ok(match &st.uid {
                Some(u) => CmdOutput::ok(format!("{u}\n")),
                None => CmdOutput::failed(1, "id: not available"),
            }),
            _ => Ok(CmdOutput::ok("")),
        }
    }

    fn which(&self, program: &str) -> Option<PathBuf> {
        self.state
            .lock()
            .unwrap()
            .present
            .contains(program)
            .then(|| PathBuf::from(format!("/fake/bin/{program}")))
    }
}
