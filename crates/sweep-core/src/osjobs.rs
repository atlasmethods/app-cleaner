//! Pure text generators for the files and command lines that put ClearSweep jobs into the
//! operating system: XDG autostart entries, systemd units, crontab lines, launchd plists and
//! Windows command lines. Nothing here touches the disk, so every format is table-tested.

use std::path::Path;

use crate::error::{ApiError, Result};

/// The executable must be an absolute, printable path without quotes: every OS scheduler
/// quotes it differently and none of them can carry the exotic cases safely.
pub fn check_exe(exe: &Path) -> Result<String> {
    let s = exe.to_str().ok_or_else(|| {
        ApiError::invalid_params("the ClearSweep executable path is not valid UTF-8")
    })?;
    if !exe.is_absolute() {
        return Err(ApiError::invalid_params(format!(
            "cannot schedule jobs: the ClearSweep executable path `{s}` is not absolute"
        )));
    }
    if s.chars().any(|c| c.is_control() || c == '"') {
        return Err(ApiError::invalid_params(
            "the ClearSweep executable path contains characters that cannot be scheduled safely",
        ));
    }
    Ok(s.to_string())
}

/// Arguments of the scheduled clean job for schedule `id`.
pub fn schedule_args(id: &str) -> Vec<String> {
    ["clean", "--auto", "--source", "scheduled", "--schedule", id]
        .into_iter()
        .map(String::from)
        .collect()
}

/// Arguments of the background agent job.
pub fn agent_args() -> Vec<String> {
    vec!["agent".to_string()]
}

// ---------------------------------------------------------------- shell / cron

/// POSIX single-quote `s` (only when needed).
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./=:,@+".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `exe args...` as one shell command line.
pub fn sh_command(exe: &str, args: &[String]) -> String {
    let mut out = sh_quote(exe);
    for a in args {
        out.push(' ');
        out.push_str(&sh_quote(a));
    }
    out
}

/// cron treats an unescaped `%` in the command as a newline.
pub fn cron_escape(cmd: &str) -> String {
    cmd.replace('%', r"\%")
}

// ---------------------------------------------------------------- systemd

/// One `ExecStart=` word: always double-quoted, with `\`, `"`, `%` and `$` escaped the way
/// systemd.service(5) expects.
pub fn systemd_word(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn systemd_exec_start(exe: &str, args: &[String]) -> String {
    let mut out = systemd_word(exe);
    for a in args {
        out.push(' ');
        out.push_str(&systemd_word(a));
    }
    out
}

/// A user-supplied name as a one-line unit `Description=` (no control characters, no
/// specifiers).
pub fn one_line(s: &str) -> String {
    plain_line(s).replace('%', "%%")
}

/// `s` on one line: control characters become spaces.
pub fn plain_line(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    t.trim().to_string()
}

// ---------------------------------------------------------------- XDG autostart

/// One word of a Desktop Entry `Exec=` line, quoted per the Desktop Entry specification
/// (double quotes around words with reserved characters; `"`, `` ` ``, `$` and `\` escaped;
/// `%` doubled).
pub fn desktop_exec_word(s: &str) -> String {
    let s = s.replace('%', "%%");
    let reserved = s.is_empty() || s.chars().any(|c| " \t\n\"'\\><~|&;$*?#()`".contains(c));
    if !reserved {
        return s;
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The whole `Exec=` value as stored in the file: the quoted command, then the file-format
/// level escaping of backslashes (a `.desktop` value is itself backslash-unescaped).
pub fn desktop_exec_value(exe: &str, args: &[String]) -> String {
    let mut cmd = desktop_exec_word(exe);
    for a in args {
        cmd.push(' ');
        cmd.push_str(&desktop_exec_word(a));
    }
    cmd.replace('\\', r"\\")
}

/// A complete autostart `.desktop` file for a background job.
pub fn desktop_entry(name: &str, comment: &str, exe: &str, args: &[String]) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nComment={}\nExec={}\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
        plain_line(name),
        plain_line(comment),
        desktop_exec_value(exe, args),
    )
}

// ---------------------------------------------------------------- launchd

pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() && c != '\t' && c != '\n' => {}
            c => out.push(c),
        }
    }
    out
}

/// One `StartCalendarInterval` entry; `None` fields are wildcards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CalendarSlot {
    pub minute: Option<u8>,
    pub hour: Option<u8>,
    /// 1 = Monday .. 7 = Sunday (launchd also accepts 0 for Sunday).
    pub weekday: Option<u8>,
    pub day: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchTrigger {
    /// Start when the agent is loaded (login).
    RunAtLoad,
    /// Start at these calendar times.
    Calendar(Vec<CalendarSlot>),
}

fn slot_dict(slot: &CalendarSlot, indent: &str) -> String {
    let mut out = format!("{indent}<dict>\n");
    for (key, v) in [
        ("Minute", slot.minute),
        ("Hour", slot.hour),
        ("Day", slot.day),
        ("Weekday", slot.weekday),
    ] {
        if let Some(v) = v {
            out.push_str(&format!(
                "{indent}\t<key>{key}</key>\n{indent}\t<integer>{v}</integer>\n"
            ));
        }
    }
    out.push_str(&format!("{indent}</dict>\n"));
    out
}

/// A launchd property list running `exe args...` for `trigger`.
pub fn launchd_plist(label: &str, exe: &str, args: &[String], trigger: &LaunchTrigger) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n",
    );
    out.push_str(&format!(
        "\t<key>Label</key>\n\t<string>{}</string>\n",
        xml_escape(label)
    ));
    out.push_str("\t<key>ProgramArguments</key>\n\t<array>\n");
    for a in std::iter::once(exe).chain(args.iter().map(String::as_str)) {
        out.push_str(&format!("\t\t<string>{}</string>\n", xml_escape(a)));
    }
    out.push_str("\t</array>\n");
    match trigger {
        LaunchTrigger::RunAtLoad => {
            out.push_str("\t<key>RunAtLoad</key>\n\t<true/>\n");
        }
        LaunchTrigger::Calendar(slots) => {
            out.push_str("\t<key>RunAtLoad</key>\n\t<false/>\n");
            out.push_str("\t<key>StartCalendarInterval</key>\n");
            if slots.len() == 1 {
                out.push_str(&slot_dict(&slots[0], "\t"));
            } else {
                out.push_str("\t<array>\n");
                for s in slots {
                    out.push_str(&slot_dict(s, "\t\t"));
                }
                out.push_str("\t</array>\n");
            }
        }
    }
    out.push_str("\t<key>KeepAlive</key>\n\t<false/>\n");
    out.push_str("\t<key>ProcessType</key>\n\t<string>Background</string>\n");
    out.push_str("</dict>\n</plist>\n");
    out
}

// ---------------------------------------------------------------- Windows

/// `"C:\Program Files\ClearSweep\clearsweep.exe" arg arg` for `/tr` and Run-key values.
/// Arguments here never contain spaces or quotes (subcommands, flags, hex ids).
pub fn windows_command(exe: &str, args: &[String]) -> String {
    let mut out = format!("\"{exe}\"");
    for a in args {
        out.push(' ');
        out.push_str(a);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_must_be_absolute_and_plain() {
        assert!(check_exe(Path::new("/usr/bin/clearsweep")).is_ok());
        assert!(check_exe(Path::new("clearsweep")).is_err());
        assert!(check_exe(Path::new("/opt/a\"b/x")).is_err());
        assert!(check_exe(Path::new("/opt/a\nb/x")).is_err());
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(sh_quote("clean"), "clean");
        assert_eq!(sh_quote("/usr/bin/clearsweep"), "/usr/bin/clearsweep");
        assert_eq!(sh_quote("/opt/My App/x"), "'/opt/My App/x'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(cron_escape("a%b"), r"a\%b");
        assert_eq!(
            sh_command("/opt/My App/cs", &schedule_args("00ff")),
            "'/opt/My App/cs' clean --auto --source scheduled --schedule 00ff"
        );
    }

    #[test]
    fn systemd_quoting() {
        assert_eq!(systemd_word("/usr/bin/cs"), "\"/usr/bin/cs\"");
        assert_eq!(systemd_word(r#"a"b\c%d$e"#), r#""a\"b\\c%%d$$e""#);
        assert_eq!(
            systemd_exec_start("/opt/My App/cs", &agent_args()),
            "\"/opt/My App/cs\" \"agent\""
        );
        assert_eq!(one_line("Nightly\n50% clean"), "Nightly 50%% clean");
    }

    #[test]
    fn desktop_exec_quoting() {
        assert_eq!(desktop_exec_word("/usr/bin/cs"), "/usr/bin/cs");
        assert_eq!(desktop_exec_word("/opt/My App/cs"), "\"/opt/My App/cs\"");
        assert_eq!(desktop_exec_word("100%"), "100%%");
        assert_eq!(desktop_exec_word("a$b"), "\"a\\$b\"");
        // Reserved character `\` needs the Exec-level escape and the file-level escape.
        assert_eq!(
            desktop_exec_value("/opt/we\\ird/cs", &agent_args()),
            "\"/opt/we\\\\\\\\ird/cs\" agent"
        );
        assert_eq!(
            desktop_exec_value("/opt/My App/cs", &agent_args()),
            "\"/opt/My App/cs\" agent"
        );
    }

    #[test]
    fn desktop_entry_shape() {
        let t = desktop_entry(
            "ClearSweep Agent",
            "Background cleaner",
            "/usr/bin/cs",
            &agent_args(),
        );
        assert_eq!(
            t,
            "[Desktop Entry]\nType=Application\nName=ClearSweep Agent\nComment=Background cleaner\nExec=/usr/bin/cs agent\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n"
        );
    }

    #[test]
    fn plist_run_at_load() {
        let t = launchd_plist(
            "app.clearsweep.agent",
            "/Applications/ClearSweep.app/Contents/MacOS/clearsweep",
            &agent_args(),
            &LaunchTrigger::RunAtLoad,
        );
        assert!(t.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist"));
        assert!(t.contains("<key>Label</key>\n\t<string>app.clearsweep.agent</string>"));
        assert!(t.contains(
            "<key>ProgramArguments</key>\n\t<array>\n\t\t<string>/Applications/ClearSweep.app/Contents/MacOS/clearsweep</string>\n\t\t<string>agent</string>\n\t</array>"
        ));
        assert!(t.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(t.contains("<key>KeepAlive</key>\n\t<false/>"));
        assert!(!t.contains("StartCalendarInterval"));
        // It must be a valid property list.
        plist::Value::from_reader_xml(t.as_bytes()).expect("valid plist");
    }

    #[test]
    fn plist_calendar_single_and_many() {
        let one = launchd_plist(
            "l",
            "/x",
            &[],
            &LaunchTrigger::Calendar(vec![CalendarSlot {
                minute: Some(30),
                hour: Some(3),
                ..Default::default()
            }]),
        );
        assert!(one.contains(
            "<key>StartCalendarInterval</key>\n\t<dict>\n\t\t<key>Minute</key>\n\t\t<integer>30</integer>\n\t\t<key>Hour</key>\n\t\t<integer>3</integer>\n\t</dict>"
        ));
        assert!(one.contains("<key>RunAtLoad</key>\n\t<false/>"));
        let many = launchd_plist(
            "l",
            "/x",
            &[],
            &LaunchTrigger::Calendar(vec![
                CalendarSlot {
                    minute: Some(0),
                    hour: Some(3),
                    weekday: Some(1),
                    ..Default::default()
                },
                CalendarSlot {
                    minute: Some(0),
                    hour: Some(3),
                    weekday: Some(3),
                    ..Default::default()
                },
            ]),
        );
        assert!(many.contains("<key>StartCalendarInterval</key>\n\t<array>\n\t\t<dict>"));
        assert_eq!(many.matches("<key>Weekday</key>").count(), 2);
        plist::Value::from_reader_xml(many.as_bytes()).expect("valid plist");
    }

    #[test]
    fn plist_escapes_xml() {
        let t = launchd_plist(
            "l",
            "/opt/A&B <x>/cs",
            &["a\"b".to_string()],
            &LaunchTrigger::RunAtLoad,
        );
        assert!(t.contains("/opt/A&amp;B &lt;x&gt;/cs"));
        assert!(t.contains("a&quot;b"));
        plist::Value::from_reader_xml(t.as_bytes()).expect("valid plist");
    }

    #[test]
    fn windows_command_line() {
        assert_eq!(
            windows_command(r"C:\Program Files\ClearSweep\clearsweep.exe", &agent_args()),
            r#""C:\Program Files\ClearSweep\clearsweep.exe" agent"#
        );
    }
}
