//! Matching startup items to running processes, and the "impact" grade.

use crate::procs::{name_matches, ProcDetail};

use super::model::Impact;

const MB: u64 = 1024 * 1024;

/// The executable of a command line (a `Run` value, a desktop `Exec=`, a cron line).
/// Handles quotes, `env VAR=x cmd`, `sh -c "cmd"` and unquoted Windows paths with spaces.
pub fn exe_from_command(cmd: &str) -> Option<String> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return None;
    }
    // Unquoted Windows path that contains spaces: `C:\Program Files\X\x.exe /background`.
    let b = cmd.as_bytes();
    let drive = b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    if drive {
        let lower = cmd.to_ascii_lowercase();
        if let Some(i) = lower.find(".exe") {
            return Some(cmd[..i + 4].to_string());
        }
    }
    let toks = split_words(cmd);
    let mut it = toks.into_iter().peekable();
    // `env`, `VAR=value` prefixes.
    while let Some(t) = it.peek() {
        let is_assign = t.contains('=') && !t.starts_with('/') && !t.starts_with('=');
        if t == "env" || is_assign {
            it.next();
        } else {
            break;
        }
    }
    let first = it.next()?;
    let base = first.rsplit(['/', '\\']).next().unwrap_or(&first);
    if matches!(base, "sh" | "bash" | "dash" | "zsh") {
        // `sh -c "real command"`
        while let Some(t) = it.next() {
            if t == "-c" {
                return it.next().and_then(|s| exe_from_command(&s));
            }
        }
        return Some(first);
    }
    // Quoted Windows command line.
    Some(first)
}

/// Whitespace split with single/double quotes (backslash escapes only inside double quotes,
/// so Windows paths survive).
pub fn split_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' && matches!(chars.peek(), Some('"') | Some('\\'))
                {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                    }
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    started = true;
                }
                c if c.is_whitespace() => {
                    if started {
                        out.push(std::mem::take(&mut cur));
                        started = false;
                    }
                }
                c => {
                    cur.push(c);
                    started = true;
                }
            },
        }
    }
    if started {
        out.push(cur);
    }
    out
}

fn file_name(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

fn parent_dir(p: &str) -> Option<&str> {
    let i = p.rfind(['/', '\\'])?;
    (i > 0).then(|| &p[..i])
}

/// Normalized application key of an executable path or process name: file name without
/// extension / `-stable` / `-bin`, lower case.
pub fn base_key(path_or_name: &str) -> String {
    let mut n = file_name(path_or_name.trim()).to_lowercase();
    for ext in [".exe", ".desktop", ".appimage", ".app", ".sh", ".py", ".bin"] {
        if let Some(s) = n.strip_suffix(ext) {
            n = s.to_string();
        }
    }
    for suf in ["-stable", "-bin"] {
        if let Some(s) = n.strip_suffix(suf) {
            n = s.to_string();
        }
    }
    n
}

fn generic_dir(dir: &str) -> bool {
    let last = file_name(dir).to_lowercase();
    matches!(
        last.as_str(),
        "bin"
            | "sbin"
            | "system32"
            | "syswow64"
            | "libexec"
            | "lib"
            | "lib64"
            | "windows"
            | "applications"
            | "program files"
            | "program files (x86)"
            | "programdata"
            | "opt"
            | "usr"
            | "local"
            | "appdata"
            | "roaming"
            | ""
    )
}

/// Does process `p` belong to the program launched by `hint` (a path or bare name)?
/// Deliberately strict: a wrong match would stop an unrelated program.
pub fn proc_matches(hint: &str, p: &ProcDetail) -> bool {
    let hk = base_key(hint);
    if hk.is_empty() {
        return false;
    }
    if let Some(exe) = &p.exe {
        if base_key(exe) == hk {
            return true;
        }
        // Helpers of the same install (`/opt/Slack/chrome_crashpad_handler`).
        if let (Some(hd), Some(pd)) = (parent_dir(hint), parent_dir(exe)) {
            if hd.eq_ignore_ascii_case(pd) && !generic_dir(hd) && (hint.starts_with('/') || hint.contains(":\\")) {
                return true;
            }
        }
    }
    base_key(&p.name) == hk || name_matches(&p.name, file_name(hint))
}

/// Grade from what the item's processes use right now; `Unknown` when it is not running.
pub fn impact_for(hint: Option<&str>, procs: &[ProcDetail]) -> Impact {
    let Some(hint) = hint else {
        return Impact::Unknown;
    };
    let mut mem = 0u64;
    let mut cpu = 0f32;
    let mut any = false;
    for p in procs.iter().filter(|p| p.is_mine && proc_matches(hint, p)) {
        any = true;
        mem += p.memory_bytes;
        cpu += p.cpu_percent;
    }
    if !any {
        Impact::Unknown
    } else if mem >= 200 * MB || cpu >= 10.0 {
        Impact::High
    } else if mem >= 50 * MB {
        Impact::Medium
    } else {
        Impact::Low
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, name: &str, exe: Option<&str>, mb: u64, cpu: f32) -> ProcDetail {
        ProcDetail {
            pid,
            name: name.into(),
            exe: exe.map(str::to_string),
            memory_bytes: mb * MB,
            cpu_percent: cpu,
            is_mine: true,
        }
    }

    #[test]
    fn exe_extraction() {
        assert_eq!(exe_from_command("/usr/bin/slack -u %U").as_deref(), Some("/usr/bin/slack"));
        assert_eq!(
            exe_from_command("env GTK_IM=x /opt/app/run --flag").as_deref(),
            Some("/opt/app/run")
        );
        assert_eq!(
            exe_from_command("sh -c \"/opt/app/run --x\"").as_deref(),
            Some("/opt/app/run")
        );
        assert_eq!(
            exe_from_command("\"C:\\Program Files\\Dropbox\\dropbox.exe\" /home").as_deref(),
            Some("C:\\Program Files\\Dropbox\\dropbox.exe")
        );
        assert_eq!(
            exe_from_command("C:\\Program Files\\Some App\\app.exe -minimized").as_deref(),
            Some("C:\\Program Files\\Some App\\app.exe")
        );
        assert_eq!(exe_from_command("   "), None);
    }

    #[test]
    fn keys_are_normalised() {
        assert_eq!(base_key("/usr/bin/Slack"), "slack");
        assert_eq!(base_key("C:\\X\\Dropbox.EXE"), "dropbox");
        assert_eq!(base_key("google-chrome-stable"), "google-chrome");
        assert_eq!(base_key("/Applications/Foo.app"), "foo");
    }

    #[test]
    fn matching_is_strict() {
        let slack = p(1, "slack", Some("/opt/Slack/slack"), 10, 0.0);
        let helper = p(2, "chrome_crashpad", Some("/opt/Slack/chrome_crashpad_handler"), 1, 0.0);
        let other = p(3, "slackware-tool", Some("/usr/bin/slackware-tool"), 1, 0.0);
        assert!(proc_matches("/opt/Slack/slack", &slack));
        assert!(proc_matches("/opt/Slack/slack", &helper));
        assert!(!proc_matches("/opt/Slack/slack", &other));
        // Generic directories never make two programs "the same app".
        let cat = p(4, "cat", Some("/usr/bin/cat"), 1, 0.0);
        assert!(!proc_matches("/usr/bin/slack", &cat));
        assert!(proc_matches("slack", &slack));
    }

    #[test]
    fn impact_grades() {
        let procs = vec![
            p(1, "big", Some("/opt/big/big"), 300, 0.0),
            p(2, "mid", Some("/opt/mid/mid"), 60, 0.0),
            p(3, "small", Some("/opt/s/small"), 5, 0.0),
            p(4, "busy", Some("/opt/b/busy"), 5, 25.0),
        ];
        assert_eq!(impact_for(Some("/opt/big/big"), &procs), Impact::High);
        assert_eq!(impact_for(Some("mid"), &procs), Impact::Medium);
        assert_eq!(impact_for(Some("small"), &procs), Impact::Low);
        assert_eq!(impact_for(Some("busy"), &procs), Impact::High);
        assert_eq!(impact_for(Some("absent"), &procs), Impact::Unknown);
        assert_eq!(impact_for(None, &procs), Impact::Unknown);
        let mut other_user = p(5, "theirs", Some("/opt/t/theirs"), 900, 0.0);
        other_user.is_mine = false;
        assert_eq!(impact_for(Some("theirs"), &[other_user]), Impact::Unknown);
    }
}
