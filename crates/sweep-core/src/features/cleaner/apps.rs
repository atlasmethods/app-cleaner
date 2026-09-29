//! Running-application detection and graceful closing.

use std::time::{Duration, Instant};

use crate::ctx::{Ctx, Os};
use crate::features::cleaner::rules::Rule;
use crate::job::Job;
use crate::procs::{name_matches, ProcInfo};

/// Processes from `procs` that indicate `rule`'s application is running.
pub fn matching_procs<'a>(rule: &Rule, os: Os, procs: &'a [ProcInfo]) -> Vec<&'a ProcInfo> {
    let want = rule.processes.for_os(os);
    if want.is_empty() {
        return Vec::new();
    }
    procs
        .iter()
        .filter(|p| want.iter().any(|w| name_matches(&p.name, w)))
        .collect()
}

pub fn is_running(rule: &Rule, os: Os, procs: &[ProcInfo]) -> bool {
    !matching_procs(rule, os, procs).is_empty()
}

/// Politely ask the app to quit: SIGTERM (Linux), `taskkill /IM` without `/F` (Windows),
/// AppleScript `quit` (macOS). Never force-kills.
pub fn request_close(ctx: &Ctx, rule: &Rule, procs: &[ProcInfo]) {
    match ctx.env.os {
        Os::Linux => {
            for p in matching_procs(rule, Os::Linux, procs) {
                ctx.procs.request_exit(p.pid);
            }
        }
        Os::Windows => {
            for name in &rule.processes.windows {
                let _ = ctx.runner.run("taskkill", &["/IM", name]);
            }
        }
        Os::MacOs => {
            for app in &rule.processes.macos {
                let script = format!("tell application \"{app}\" to quit");
                let _ = ctx.runner.run("osascript", &["-e", &script]);
            }
        }
    }
}

/// Ask `rule`'s app to close and wait up to `timeout` for it to exit.
/// Returns the refreshed process list and whether the app is gone.
pub fn close_and_wait(
    ctx: &Ctx,
    rule: &Rule,
    procs: &[ProcInfo],
    timeout: Duration,
    job: &Job,
) -> (Vec<ProcInfo>, bool) {
    request_close(ctx, rule, procs);
    let deadline = Instant::now() + timeout;
    loop {
        let now = ctx.procs.list();
        if !is_running(rule, ctx.env.os, &now) {
            return (now, true);
        }
        if Instant::now() >= deadline || job.is_cancelled() {
            return (now, false);
        }
        std::thread::sleep(Duration::from_millis(100).min(timeout));
    }
}
