//! Running `reg.exe`: directly for per-user keys, and in batches (ONE administrator prompt per
//! batch) for machine-wide keys.
//!
//! The batch is a single PowerShell command line handed to
//! [`run_privileged`](crate::elevate::run_privileged). Nothing is read from a file the
//! unprivileged user could swap between the prompt and the run: the commands travel inside the
//! (authorised) command line itself. The script prints `R<index>:<exit code>:<message>` per
//! command so every issue gets its own result.
//!
//! Limits: the elevated command line passes through `cmd.exe` (8191 characters), and
//! [`crate::elevate`] refuses `"` and `%` in arguments, so scripts avoid both and batches are
//! split by length.

use crate::ctx::Ctx;
use crate::elevate::run_privileged;
use crate::error::Result;
use crate::pkgutil::summarize;

/// One `reg.exe` invocation (arguments after the program name).
#[derive(Debug, Clone, PartialEq)]
pub struct RegCmd {
    pub args: Vec<String>,
}

impl RegCmd {
    pub fn new(args: &[&str]) -> RegCmd {
        RegCmd {
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }
    pub fn delete_key(path: &str) -> RegCmd {
        RegCmd::new(&["delete", path, "/f"])
    }
    /// `name` empty = the key's default value.
    pub fn delete_value(path: &str, name: &str) -> RegCmd {
        if name.is_empty() {
            RegCmd::new(&["delete", path, "/ve", "/f"])
        } else {
            RegCmd::new(&["delete", path, "/v", name, "/f"])
        }
    }
    pub fn import(file: &str) -> RegCmd {
        RegCmd::new(&["import", file])
    }
}

/// Outcome of one command.
#[derive(Debug, Clone, PartialEq)]
pub struct CmdResult {
    pub ok: bool,
    pub message: String,
}

impl CmdResult {
    fn ok() -> CmdResult {
        CmdResult {
            ok: true,
            message: String::new(),
        }
    }
    fn fail(m: impl Into<String>) -> CmdResult {
        CmdResult {
            ok: false,
            message: m.into(),
        }
    }
}

/// Run one command without elevation.
pub fn run_direct(ctx: &Ctx, cmd: &RegCmd) -> CmdResult {
    let args: Vec<&str> = cmd.args.iter().map(String::as_str).collect();
    match ctx.runner.run("reg", &args) {
        Ok(o) if o.success() => CmdResult::ok(),
        Ok(o) => CmdResult::fail(format!("reg {} failed: {}", cmd.args[0], summarize(&o))),
        Err(e) => CmdResult::fail(e.message),
    }
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// `"C:\Program Files\Foo\"` must reach `reg.exe` as one argument. Windows PowerShell 5.1 wraps
/// an argument containing spaces in double quotes without escaping a trailing backslash, which
/// would swallow the closing quote, so those backslashes are doubled (an argument without
/// spaces is passed bare and is left alone).
pub fn ps_native_arg(a: &str) -> String {
    if a.ends_with('\\') && a.contains(char::is_whitespace) {
        format!(
            "{a}{}",
            "\\".repeat(a.len() - a.trim_end_matches('\\').len())
        )
    } else {
        a.to_string()
    }
}

const SCRIPT_HEAD: &str = "function r($i,$a){$o=(& reg @a 2>&1|Out-String).Trim() -replace '\\s+',' ';'R'+$i+':'+$LASTEXITCODE+':'+$o};";
/// Longest script sent in one elevated call (the total command line is limited to 8191).
const MAX_SCRIPT: usize = 6000;

/// The PowerShell text for one batch. Commands with `"` or `%` in an argument cannot be sent
/// (returns `None` for those indexes' text; callers filter them beforehand).
pub fn batch_script(cmds: &[RegCmd]) -> String {
    let mut s = String::from(SCRIPT_HEAD);
    for (i, c) in cmds.iter().enumerate() {
        let list: Vec<String> = c.args.iter().map(|a| ps_quote(&ps_native_arg(a))).collect();
        s.push_str(&format!("r {i} @({});", list.join(",")));
    }
    s
}

/// Can this command travel through the elevated command line?
pub fn batchable(c: &RegCmd) -> bool {
    c.args
        .iter()
        .all(|a| !a.contains(['"', '%', '\n', '\r', '\0']))
}

/// Split into groups whose script stays under the length limit.
fn chunks(cmds: &[RegCmd]) -> Vec<&[RegCmd]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut len = SCRIPT_HEAD.len();
    for (i, c) in cmds.iter().enumerate() {
        let one = batch_script(std::slice::from_ref(c)).len() - SCRIPT_HEAD.len() + 4;
        if i > start && len + one > MAX_SCRIPT {
            out.push(&cmds[start..i]);
            start = i;
            len = SCRIPT_HEAD.len();
        }
        len += one;
    }
    if start < cmds.len() {
        out.push(&cmds[start..]);
    }
    out
}

/// Parse `R<i>:<code>:<message>` lines.
pub fn parse_batch_output(out: &str, n: usize) -> Vec<Option<CmdResult>> {
    let mut v: Vec<Option<CmdResult>> = vec![None; n];
    for line in out.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix('R') else {
            continue;
        };
        let mut it = rest.splitn(3, ':');
        let (Some(i), Some(code)) = (it.next(), it.next()) else {
            continue;
        };
        let (Ok(i), Ok(code)) = (i.parse::<usize>(), code.trim().parse::<i32>()) else {
            continue;
        };
        if i < n {
            let msg = it.next().unwrap_or("").trim();
            v[i] = Some(if code == 0 {
                CmdResult::ok()
            } else {
                CmdResult::fail(if msg.is_empty() {
                    format!("reg exited with code {code}")
                } else {
                    msg.to_string()
                })
            });
        }
    }
    v
}

/// Run every command elevated, one administrator prompt per chunk. A declined prompt fails the
/// remaining commands with the reason; commands that cannot be sent get their own failure.
pub fn run_privileged_batch(ctx: &Ctx, cmds: &[RegCmd]) -> Vec<CmdResult> {
    let mut results: Vec<Option<CmdResult>> = vec![None; cmds.len()];
    let sendable: Vec<usize> = (0..cmds.len()).filter(|i| batchable(&cmds[*i])).collect();
    for (i, slot) in results.iter_mut().enumerate() {
        if !sendable.contains(&i) {
            *slot = Some(CmdResult::fail(
                "this entry contains a quote or percent sign and cannot be changed with administrator rights",
            ));
        }
    }
    let list: Vec<RegCmd> = sendable.iter().map(|i| cmds[*i].clone()).collect();
    let mut pos = 0usize;
    let mut abort: Option<String> = None;
    for chunk in chunks(&list) {
        let mut chunk_results: Vec<Option<CmdResult>> = vec![None; chunk.len()];
        match &abort {
            Some(reason) => {
                chunk_results = vec![Some(CmdResult::fail(reason.clone())); chunk.len()];
            }
            None => {
                let script = batch_script(chunk);
                let r: Result<_> = run_privileged(
                    ctx,
                    "powershell",
                    &["-NoProfile", "-NonInteractive", "-Command", &script],
                );
                match r {
                    Ok(o) => {
                        chunk_results = parse_batch_output(&o.stdout, chunk.len());
                        if chunk_results.iter().all(Option::is_none) {
                            let why = if o.success() {
                                "the elevated run reported nothing".to_string()
                            } else {
                                format!("the elevated run failed: {}", summarize(&o))
                            };
                            chunk_results = vec![Some(CmdResult::fail(why)); chunk.len()];
                        }
                    }
                    Err(e) => {
                        abort = Some(e.message.clone());
                        chunk_results = vec![Some(CmdResult::fail(e.message)); chunk.len()];
                    }
                }
            }
        }
        for r in chunk_results {
            results[sendable[pos]] = r;
            pos += 1;
        }
    }
    results
        .into_iter()
        .map(|r| r.unwrap_or_else(|| CmdResult::fail("no result was reported for this entry")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elevate::{powershell_decode, with_elevation};
    use crate::runner::{CmdOutput, MockRunner};

    fn ctx() -> (tempfile::TempDir, Ctx, MockRunner) {
        let d = tempfile::tempdir().unwrap();
        let m = MockRunner::new();
        let mut c = Ctx::test(d.path(), m.clone());
        c.env.os = crate::ctx::Os::Windows;
        (d, c, m)
    }

    #[test]
    fn commands() {
        assert_eq!(
            RegCmd::delete_value(r"HKLM\A", "n").args,
            ["delete", r"HKLM\A", "/v", "n", "/f"]
        );
        assert_eq!(
            RegCmd::delete_value(r"HKLM\A", "").args,
            ["delete", r"HKLM\A", "/ve", "/f"]
        );
        assert_eq!(
            RegCmd::delete_key(r"HKCU\A").args,
            ["delete", r"HKCU\A", "/f"]
        );
        assert_eq!(RegCmd::import(r"C:\b.reg").args, ["import", r"C:\b.reg"]);
    }

    #[test]
    fn trailing_backslash_handling() {
        assert_eq!(
            ps_native_arg(r"C:\Program Files\Foo\"),
            r"C:\Program Files\Foo\\"
        );
        assert_eq!(
            ps_native_arg(r"C:\Program Files\Foo\\"),
            r"C:\Program Files\Foo\\\\"
        );
        assert_eq!(ps_native_arg(r"C:\Foo\"), r"C:\Foo\"); // no spaces: passed bare
        assert_eq!(
            ps_native_arg(r"C:\Program Files\a.dll"),
            r"C:\Program Files\a.dll"
        );
    }

    #[test]
    fn script_shape() {
        let s = batch_script(&[
            RegCmd::delete_value(r"HKLM\SOFTWARE\A B", "it's"),
            RegCmd::delete_key(r"HKLM\SOFTWARE\C"),
        ]);
        assert!(s.starts_with("function r("));
        assert!(s.contains(r"r 0 @('delete','HKLM\SOFTWARE\A B','/v','it''s','/f');"));
        assert!(s.contains(r"r 1 @('delete','HKLM\SOFTWARE\C','/f');"));
        assert!(!s.contains('"') && !s.contains('%') && !s.contains('\n'));
    }

    #[test]
    fn output_parsing() {
        let v = parse_batch_output(
            "noise\nR0:0:\nR1:1:ERROR: Access is denied.\nR2:x:bad\nR9:0:\nR3:1:\n",
            4,
        );
        assert_eq!(v[0], Some(CmdResult::ok()));
        assert_eq!(v[1].as_ref().unwrap().message, "ERROR: Access is denied.");
        assert!(!v[1].as_ref().unwrap().ok);
        assert_eq!(v[2], None);
        assert_eq!(v[3].as_ref().unwrap().message, "reg exited with code 1");
    }

    #[test]
    fn chunking_respects_the_limit() {
        let cmds: Vec<RegCmd> = (0..200)
            .map(|i| {
                RegCmd::delete_value(
                    r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\SharedDLLs",
                    &format!(r"C:\Program Files\Common Files\Some Vendor\Library{i}.dll"),
                )
            })
            .collect();
        let ch = chunks(&cmds);
        assert!(ch.len() > 1);
        assert_eq!(ch.iter().map(|c| c.len()).sum::<usize>(), 200);
        for c in &ch {
            assert!(
                batch_script(c).len() <= MAX_SCRIPT,
                "{}",
                batch_script(c).len()
            );
        }
        assert!(chunks(&[]).is_empty());
    }

    #[test]
    fn batch_runs_once_per_chunk_and_maps_results() {
        with_elevation(true, || {
            let (_d, c, m) = ctx();
            m.on_any_args("powershell", CmdOutput::ok("R0:0:\nR1:1:ERROR: The system was unable to find the specified registry key or value.\nR2:0:\n"));
            let cmds = vec![
                RegCmd::delete_key(r"HKLM\SOFTWARE\A"),
                RegCmd::delete_key(r"HKLM\SOFTWARE\B"),
                RegCmd::delete_key(r"HKLM\SOFTWARE\C"),
            ];
            let r = run_privileged_batch(&c, &cmds);
            assert_eq!(m.calls().len(), 1);
            assert!(r[0].ok && !r[1].ok && r[2].ok);
            assert!(r[1].message.contains("unable to find"));
            let (prog, args) = &m.calls()[0];
            assert_eq!(prog, "powershell");
            assert_eq!(args[2], "-Command");
            assert!(args[3].contains(r"r 2 @('delete','HKLM\SOFTWARE\C','/f');"));
        });
    }

    #[test]
    fn unelevated_batch_goes_through_the_runas_wrapper() {
        with_elevation(false, || {
            let (_d, c, m) = ctx();
            m.on_any_args("powershell", CmdOutput::ok("R0:0:\n"));
            let r = run_privileged_batch(&c, &[RegCmd::delete_key(r"HKLM\SOFTWARE\A")]);
            assert!(r[0].ok);
            let calls = m.calls();
            assert_eq!(calls.len(), 1);
            let enc = calls[0].1.last().unwrap();
            let script = powershell_decode(enc).unwrap();
            assert!(script.contains("-Verb RunAs"));
            assert!(script.contains("HKLM"));
        });
    }

    #[test]
    fn unsendable_commands_fail_individually() {
        with_elevation(true, || {
            let (_d, c, m) = ctx();
            m.on_any_args("powershell", CmdOutput::ok("R0:0:\n"));
            let cmds = vec![
                RegCmd::delete_value(r"HKLM\SOFTWARE\A", "50%"),
                RegCmd::delete_value(r"HKLM\SOFTWARE\A", "ok"),
                RegCmd::delete_value(r"HKLM\SOFTWARE\A", "q\"uote"),
            ];
            let r = run_privileged_batch(&c, &cmds);
            assert!(!r[0].ok && r[1].ok && !r[2].ok);
            assert!(r[0].message.contains("quote or percent"));
        });
    }

    #[test]
    fn declined_prompt_fails_everything_after_it() {
        with_elevation(false, || {
            let (_d, c, m) = ctx();
            m.on_any_args(
                "powershell",
                CmdOutput {
                    status: 1223,
                    stdout: String::new(),
                    stderr: format!("{}\n", crate::elevate::WIN_CANCEL_MARKER),
                },
            );
            let cmds: Vec<RegCmd> = (0..120)
                .map(|i| {
                    RegCmd::delete_value(
                        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\SharedDLLs",
                        &format!(r"C:\Program Files\Common Files\Some Vendor\Library{i}.dll"),
                    )
                })
                .collect();
            let r = run_privileged_batch(&c, &cmds);
            assert!(r.iter().all(|x| !x.ok));
            // asked once, then gave up instead of prompting again for every chunk
            assert_eq!(m.calls().len(), 1);
            assert!(r[119].message.contains("cancelled"));
        });
    }

    #[test]
    fn silent_or_failed_run_is_a_failure_not_a_success() {
        with_elevation(true, || {
            let (_d, c, m) = ctx();
            m.on_any_args("powershell", CmdOutput::ok("nothing useful\n"));
            let r = run_privileged_batch(&c, &[RegCmd::delete_key(r"HKLM\SOFTWARE\A")]);
            assert!(!r[0].ok);
            let (_d, c, m) = ctx();
            m.on_any_args("powershell", CmdOutput::failed(1, "boom"));
            let r = run_privileged_batch(&c, &[RegCmd::delete_key(r"HKLM\SOFTWARE\A")]);
            assert!(!r[0].ok && r[0].message.contains("boom"));
        });
    }

    #[test]
    fn direct_run_reports_failures() {
        let (_d, c, m) = ctx();
        m.on("reg", &["delete", r"HKCU\A", "/f"], CmdOutput::ok(""));
        m.on(
            "reg",
            &["delete", r"HKCU\B", "/f"],
            CmdOutput::failed(1, "ERROR: Access is denied."),
        );
        assert!(run_direct(&c, &RegCmd::delete_key(r"HKCU\A")).ok);
        let r = run_direct(&c, &RegCmd::delete_key(r"HKCU\B"));
        assert!(!r.ok && r.message.contains("Access is denied"));
        assert!(!run_direct(&c, &RegCmd::delete_key(r"HKCU\C")).ok); // unscripted -> error
    }
}
