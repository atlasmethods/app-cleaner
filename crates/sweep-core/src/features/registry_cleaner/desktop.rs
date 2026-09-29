//! freedesktop.org Desktop Entry parsing: just enough to find the program a launcher starts.
//!
//! Follows the Desktop Entry specification: `Exec` is a string value (`\s \n \t \r \\` escapes)
//! that is then split into arguments with double-quote quoting (inside quotes `\" \` \$ \\` are
//! escapes), and field codes (`%f %F %u %U %i %c %k ...`) are placeholders, not text.

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub exec: Option<String>,
    pub try_exec: Option<String>,
    pub path: Option<String>,
}

/// Parse the `[Desktop Entry]` group (other groups, e.g. `[Desktop Action x]`, are ignored).
pub fn parse(content: &str) -> Entry {
    let mut e = Entry::default();
    let mut in_main = false;
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if k.contains('[') {
            continue; // localized keys
        }
        let slot = match k {
            "Name" => &mut e.name,
            "Type" => &mut e.kind,
            "Exec" => &mut e.exec,
            "TryExec" => &mut e.try_exec,
            "Path" => &mut e.path,
            _ => continue,
        };
        if slot.is_none() {
            *slot = Some(v.to_string());
        }
    }
    e
}

/// Undo the value-level escapes (`\s`, `\n`, `\t`, `\r`, `\\`).
pub fn unescape_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Split an `Exec` value into arguments (quotes removed, escapes resolved, field codes kept).
/// `None` for an unterminated quote.
pub fn split_exec(exec: &str) -> Option<Vec<String>> {
    let s = unescape_value(exec);
    let mut args: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut in_quote = false;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if in_quote {
            match c {
                '"' => in_quote = false,
                '\\' => match it.peek() {
                    Some('"' | '`' | '$' | '\\') => cur.push(it.next().unwrap()),
                    _ => cur.push('\\'),
                },
                c => cur.push(c),
            }
        } else if c == '"' {
            in_quote = true;
            started = true;
        } else if c.is_whitespace() {
            if started {
                args.push(std::mem::take(&mut cur));
                started = false;
            }
        } else {
            cur.push(c);
            started = true;
        }
    }
    if in_quote {
        return None;
    }
    if started {
        args.push(cur);
    }
    Some(args)
}

/// Remove field codes: `%%` becomes `%`, `%f`, `%U`... vanish, and an argument that consisted of
/// nothing else disappears.
pub fn strip_field_codes(args: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for a in args {
        let mut s = String::new();
        let mut had_code = false;
        let mut it = a.chars().peekable();
        while let Some(c) = it.next() {
            if c == '%' {
                match it.peek() {
                    Some('%') => {
                        it.next();
                        s.push('%');
                    }
                    Some(n) if "fFuUdDnNickvm".contains(*n) => {
                        it.next();
                        had_code = true;
                    }
                    _ => s.push('%'),
                }
            } else {
                s.push(c);
            }
        }
        if s.is_empty() && had_code {
            continue;
        }
        out.push(s);
    }
    out
}

/// What a launcher really starts.
#[derive(Debug, Clone, PartialEq)]
pub enum ExecTarget {
    Program(String),
    /// `flatpak run ... org.example.App`
    Flatpak(String),
    /// Too exotic to judge.
    Unknown,
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn is_env_assignment(a: &str) -> bool {
    match a.split_once('=') {
        Some((k, _)) => {
            let mut c = k.chars();
            c.next().is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
                && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        }
        None => false,
    }
}

pub fn is_flatpak_app_id(s: &str) -> bool {
    s.len() <= 255
        && s.contains('.')
        && s.split('.').all(|p| {
            !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

/// Look through `env [VAR=x]... prog` and `flatpak run app`.
pub fn exec_target(args: &[String]) -> ExecTarget {
    let mut i = 0;
    if args.first().is_some_and(|a| basename(a) == "env") {
        i = 1;
        while i < args.len() {
            let a = &args[i];
            if a == "-i" || a == "--ignore-environment" {
                i += 1;
            } else if is_env_assignment(a) {
                i += 1;
            } else if a.starts_with('-') {
                return ExecTarget::Unknown; // -u, -S, -C, ...: not judged
            } else {
                break;
            }
        }
    }
    let Some(prog) = args.get(i) else {
        return ExecTarget::Unknown;
    };
    if basename(prog) == "flatpak" && args.get(i + 1).map(String::as_str) == Some("run") {
        // Options that take their value in the next argument are not handled: give up.
        for a in &args[i + 2..] {
            if a.starts_with("--") && a.contains('=') {
                continue;
            }
            if matches!(a.as_str(), "--user" | "--system" | "--devel" | "-d" | "--sandbox") {
                continue;
            }
            if a.starts_with('-') {
                return ExecTarget::Unknown;
            }
            return if is_flatpak_app_id(a) {
                ExecTarget::Flatpak(a.clone())
            } else {
                ExecTarget::Unknown
            };
        }
        return ExecTarget::Unknown;
    }
    ExecTarget::Program(prog.clone())
}

/// `Exec` / `TryExec` of a parsed entry -> target.
pub fn target_of_exec(exec: &str) -> ExecTarget {
    match split_exec(exec) {
        Some(args) => exec_target(&strip_field_codes(args)),
        None => ExecTarget::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        strip_field_codes(split_exec(s).unwrap())
    }

    #[test]
    fn parses_the_main_group_only() {
        let e = parse(
            "# comment\n[Desktop Entry]\nType=Application\nName=Foo\nName[de]=Fuu\nExec=foo %U\nTryExec = /usr/bin/foo \nPath=/opt/foo\n\n[Desktop Action new]\nExec=other\n",
        );
        assert_eq!(e.name.as_deref(), Some("Foo"));
        assert_eq!(e.kind.as_deref(), Some("Application"));
        assert_eq!(e.exec.as_deref(), Some("foo %U"));
        assert_eq!(e.try_exec.as_deref(), Some("/usr/bin/foo"));
        assert_eq!(e.path.as_deref(), Some("/opt/foo"));
        // first definition wins
        assert_eq!(parse("[Desktop Entry]\nExec=a\nExec=b\n").exec.as_deref(), Some("a"));
        assert_eq!(parse("Exec=a\n").exec, None);
    }

    #[test]
    fn field_codes_are_placeholders() {
        assert_eq!(args("foo %U"), ["foo"]);
        assert_eq!(args("foo %f %F %u %d %D %n %N %i %c %k %v %m --x"), ["foo", "--x"]);
        assert_eq!(args("foo --file=%f"), ["foo", "--file="]);
        assert_eq!(args("foo 100%% sure"), ["foo", "100%", "sure"]);
        assert_eq!(args("foo --icon %i --name %c"), ["foo", "--icon", "--name"]);
    }

    #[test]
    fn quoting_and_escapes() {
        assert_eq!(args(r#""/opt/My App/bin/app" --flag %F"#), ["/opt/My App/bin/app", "--flag"]);
        assert_eq!(args(r#"sh -c "echo \"hi\" \$HOME""#), ["sh", "-c", r#"echo "hi" $HOME"#]);
        // the value-level `\s` is applied first, so it can be part of a quoted argument...
        assert_eq!(args(r#""/opt/a\sb/app" x"#), ["/opt/a b/app", "x"]);
        // ...and separates arguments when it is not quoted
        assert_eq!(args(r"/opt/a\sb/app x"), ["/opt/a", "b/app", "x"]);
        assert_eq!(args(r#""a b" "" c"#), ["a b", "", "c"]);
        assert!(split_exec(r#""unterminated"#).is_none());
        assert_eq!(unescape_value(r"a\\b\sc\nd\q"), "a\\b c\nd\\q");
        // level-2 escape of a backslash inside quotes
        assert_eq!(args(r#""a\\\\b""#), [r"a\b"]);
    }

    #[test]
    fn env_wrapper() {
        let t = |s: &str| exec_target(&args(s));
        assert_eq!(t("env FOO=bar /usr/bin/prog %U"), ExecTarget::Program("/usr/bin/prog".into()));
        assert_eq!(t("/usr/bin/env A=1 B=2 prog"), ExecTarget::Program("prog".into()));
        assert_eq!(t("env -i A=1 prog"), ExecTarget::Program("prog".into()));
        assert_eq!(t("env -u FOO prog"), ExecTarget::Unknown);
        assert_eq!(t("env"), ExecTarget::Unknown);
        assert_eq!(t("env FOO=bar"), ExecTarget::Unknown);
        assert_eq!(t("prog A=1"), ExecTarget::Program("prog".into()));
    }

    #[test]
    fn flatpak_and_snap_wrappers() {
        let t = |s: &str| exec_target(&args(s));
        assert_eq!(
            t("/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=gimp org.gimp.GIMP @@u %U @@"),
            ExecTarget::Flatpak("org.gimp.GIMP".into())
        );
        assert_eq!(t("flatpak run org.mozilla.firefox"), ExecTarget::Flatpak("org.mozilla.firefox".into()));
        assert_eq!(t("flatpak run --command foo org.x.Y"), ExecTarget::Unknown);
        assert_eq!(t("flatpak run notanid"), ExecTarget::Unknown);
        assert_eq!(t("flatpak run"), ExecTarget::Unknown);
        assert_eq!(t("flatpak list"), ExecTarget::Program("flatpak".into()));
        assert_eq!(t("env BAMF_DESKTOP_FILE_HINT=/x.desktop /snap/bin/foo %U"), ExecTarget::Program("/snap/bin/foo".into()));
        assert_eq!(t("snap run foo"), ExecTarget::Program("snap".into()));
        assert!(is_flatpak_app_id("org.gimp.GIMP"));
        assert!(!is_flatpak_app_id("gimp"));
        assert!(!is_flatpak_app_id("org..x"));
        assert!(!is_flatpak_app_id("org.x/../y"));
    }

    #[test]
    fn target_of_exec_handles_garbage() {
        assert_eq!(target_of_exec(""), ExecTarget::Unknown);
        assert_eq!(target_of_exec("%U"), ExecTarget::Unknown);
        assert_eq!(target_of_exec("\"unterminated"), ExecTarget::Unknown);
        assert_eq!(target_of_exec("foo %U"), ExecTarget::Program("foo".into()));
    }
}
