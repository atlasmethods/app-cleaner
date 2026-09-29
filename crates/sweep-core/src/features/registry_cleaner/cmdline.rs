//! Working out which file a registry value points at, and whether that file is really gone.
//!
//! False positives here would damage users' systems, so the contract is deliberately one-sided:
//! [`Located::Missing`] is only returned when the target is unambiguous AND the probe said
//! "not found" for every plausible spelling of it. Everything else is `Present` or `Unknown`
//! and is never reported:
//!
//! * unresolved `%VARS%`, UNC paths, relative paths, wildcards, odd characters -> `Unknown`
//! * bare program names (`notepad.exe`) are searched in the system directories and `PATH`; not
//!   found there is still `Unknown`, because App Paths and per-user PATH entries exist
//! * paths on removable / network drives, or on a volume that cannot be read -> `Unknown`
//! * a path counts as present if it exists as written OR under WOW64 redirection
//!   (`System32` <-> `SysWOW64`, `Program Files` <-> `Program Files (x86)`) OR with `.exe` added
//!   (what `CreateProcess` does for names without an extension)
//! * an unquoted path with spaces is resolved by probing every prefix that ends in an
//!   executable extension; it is only `Missing` when none exists
//! * a launcher (`cmd /c`, `powershell`, `msiexec`, ...) is judged by the launcher itself; only
//!   `rundll32 lib.dll,Entry` is followed to its library

use super::regaccess::{Stat, WinProbe};

/// Verdict on a path or command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Located {
    /// The target exists (in some spelling).
    Present,
    /// Unambiguously named, and gone. Carries the path (for messages).
    Missing(String),
    /// Cannot tell; the caller must not report it.
    Unknown,
}

impl Located {
    pub fn is_missing(&self) -> bool {
        matches!(self, Located::Missing(_))
    }
}

/// Extensions that mark the end of a program / library path in a command line.
const KNOWN_EXTS: &[&str] = &[
    "exe", "com", "bat", "cmd", "scr", "cpl", "msc", "lnk", "dll", "sys", "ocx", "vbs", "wsf",
    "js", "ps1", "msi", "tlb", "olb", "wav", "hlp", "chm", "ttf", "ttc", "otf", "fon", "ax", "drv",
    "mui",
];

fn has_known_ext(token: &str) -> bool {
    let name = token.rsplit(['\\', '/']).next().unwrap_or(token);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => {
            KNOWN_EXTS.contains(&ext.to_ascii_lowercase().as_str())
        }
        _ => false,
    }
}

fn last_segment_has_dot(path: &str) -> bool {
    path.rsplit('\\').next().unwrap_or(path).contains('.')
}

// ---------------------------------------------------------------- environment

/// The Windows directory (`C:\Windows`), without a trailing backslash.
pub fn windir<P: WinProbe + ?Sized>(p: &P) -> Option<String> {
    p.env_var("SystemRoot")
        .or_else(|| p.env_var("windir"))
        .map(|w| w.trim_end_matches(['\\', '/']).to_string())
        .filter(|w| !w.is_empty())
}

/// Expand `%NAME%` references. Unresolvable references are left as they are (the caller then
/// treats a path containing `%` as unknown), and a `%` that cannot start a reference stays literal.
pub fn expand_env<P: WinProbe + ?Sized>(p: &P, s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let name_end = after.find('%');
        match name_end {
            Some(j)
                if j > 0
                    && !after[..j]
                        .chars()
                        .any(|c| c.is_whitespace() || c == '\\' || c == '"') =>
            {
                let name = &after[..j];
                match p.env_var(name) {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len()
        && s.is_char_boundary(prefix.len())
        && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// Trim, expand environment variables and rewrite the NT-style prefixes services use
/// (`\SystemRoot\`, `SystemRoot\`, `\??\`, `\\?\`).
fn prepare<P: WinProbe + ?Sized>(p: &P, raw: &str) -> Option<String> {
    let s = raw.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if s.is_empty() {
        return None;
    }
    let mut s = expand_env(p, s);
    if let Some(r) = s.strip_prefix("\\??\\") {
        s = r.to_string();
    } else if let Some(r) = s.strip_prefix("\\\\?\\") {
        if r.len() >= 2 && r.as_bytes()[1] == b':' {
            s = r.to_string();
        } else {
            return None; // \\?\UNC\... or a volume GUID path
        }
    }
    for prefix in ["\\SystemRoot\\", "SystemRoot\\"] {
        if starts_with_ci(&s, prefix) {
            let w = windir(p)?;
            s = format!("{w}\\{}", &s[prefix.len()..]);
            break;
        }
    }
    Some(s)
}

// ---------------------------------------------------------------- classification

fn drive_letter(path: &str) -> Option<char> {
    let b = path.as_bytes();
    (b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic()).then(|| b[0] as char)
}

/// Other spellings under which a 32-bit or 64-bit program would find the same file.
fn alternates(path: &str, windir: Option<&str>, try_exe: bool) -> Vec<String> {
    let mut v = vec![path.to_string()];
    let segs: Vec<&str> = path.split('\\').collect();
    let swap = |from: &str, to: &[&str], idx: Option<usize>, v: &mut Vec<String>| {
        for (i, s) in segs.iter().enumerate() {
            if s.eq_ignore_ascii_case(from) && idx.is_none_or(|w| w == i) {
                for t in to {
                    let mut c = segs.clone();
                    c[i] = t;
                    v.push(c.join("\\"));
                }
                break;
            }
        }
    };
    // C:\Windows\System32 <-> SysWOW64 (and the 32-bit-only `sysnative` alias)
    let win_idx = windir.map(|w| w.split('\\').count()); // index of the directory after windir
    if let Some(idx) = win_idx {
        let in_win = windir.is_some_and(|w| starts_with_ci(path, w));
        if in_win {
            swap("system32", &["SysWOW64", "sysnative"], Some(idx), &mut v);
            swap("syswow64", &["System32"], Some(idx), &mut v);
        }
    }
    swap("Program Files (x86)", &["Program Files"], Some(1), &mut v);
    swap("Program Files", &["Program Files (x86)"], Some(1), &mut v);
    if try_exe && !last_segment_has_dot(path) {
        let extra: Vec<String> = v.iter().map(|a| format!("{a}.exe")).collect();
        v.extend(extra);
    }
    v
}

fn is_bare_name(c: &str) -> bool {
    !c.is_empty()
        && c.chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "._+()-".contains(ch))
}

/// Classify one fully-expanded candidate path.
fn classify<P: WinProbe + ?Sized>(p: &P, cand: &str, try_exe: bool) -> Located {
    let c = cand.trim();
    if c.is_empty()
        || c.contains('%')
        || c.chars()
            .any(|ch| ch.is_control() || "<>|*?\"".contains(ch))
    {
        return Located::Unknown;
    }
    let c = if drive_letter(c).is_some() {
        c.replace('/', "\\")
    } else {
        c.to_string()
    };
    if c.starts_with("\\\\") {
        return Located::Unknown; // UNC
    }
    let w = windir(p);
    if let Some(letter) = drive_letter(&c) {
        if c.len() < 3 || c.as_bytes()[2] != b'\\' {
            return Located::Unknown; // "C:" or "C:relative"
        }
        if c.contains("\\..\\") || c.ends_with("\\..") || c.contains("\\.\\") {
            return Located::Unknown;
        }
        // The volume itself must be readable.
        if p.stat(&format!("{letter}:\\")) != Stat::Dir {
            return Located::Unknown;
        }
        let mut unknown = false;
        for a in alternates(&c, w.as_deref(), try_exe) {
            match p.stat(&a) {
                Stat::File | Stat::Dir => return Located::Present,
                Stat::Unknown => unknown = true,
                Stat::Missing => {}
            }
        }
        if unknown || !p.is_fixed_drive(letter) {
            return Located::Unknown;
        }
        return Located::Missing(c.trim_end_matches('\\').to_string());
    }
    if c.contains('\\') || c.contains('/') {
        // Relative with a separator: only meaningful relative to the Windows directory
        // (`system32\drivers\x.sys`), and never "missing".
        if c.starts_with(['\\', '/', '.']) {
            return Located::Unknown;
        }
        let Some(w) = w else {
            return Located::Unknown;
        };
        let full = format!("{w}\\{}", c.replace('/', "\\"));
        for a in alternates(&full, Some(&w), try_exe) {
            if matches!(p.stat(&a), Stat::File | Stat::Dir) {
                return Located::Present;
            }
        }
        return Located::Unknown;
    }
    // A bare name: look where Windows would.
    if !is_bare_name(&c) {
        return Located::Unknown;
    }
    let mut dirs: Vec<String> = Vec::new();
    if let Some(w) = &w {
        dirs.push(format!("{w}\\System32"));
        dirs.push(format!("{w}\\SysWOW64"));
        dirs.push(w.clone());
    }
    if let Some(path) = p.env_var("PATH") {
        for d in path.split(';') {
            let d = expand_env(p, d.trim().trim_matches('"'));
            if !d.is_empty() && !d.contains('%') {
                dirs.push(d.trim_end_matches('\\').to_string());
            }
        }
    }
    let with_exe = format!("{c}.exe");
    for d in dirs {
        for name in [c.as_str(), with_exe.as_str()] {
            if name == with_exe && (last_segment_has_dot(&c) || !try_exe) {
                continue;
            }
            if matches!(p.stat(&format!("{d}\\{name}")), Stat::File | Stat::Dir) {
                return Located::Present;
            }
        }
    }
    Located::Unknown
}

// ---------------------------------------------------------------- public entry points

/// Judge a plain path value (no arguments): a shared DLL, a help file, a sound, ...
pub fn locate_path<P: WinProbe + ?Sized>(p: &P, raw: &str) -> Located {
    let Some(s) = prepare(p, raw) else {
        return Located::Unknown;
    };
    let s = if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s
    };
    if s.contains(';') {
        return Located::Unknown;
    }
    classify(p, &s, false)
}

fn basename_lower(path: &str) -> String {
    path.rsplit(['\\', '/'])
        .next()
        .unwrap_or(path)
        .trim_matches('"')
        .to_ascii_lowercase()
}

/// `rundll32 [switches] lib.dll,Entry args`: the library is what has to exist.
fn locate_rundll_target<P: WinProbe + ?Sized>(p: &P, rest: &str) -> Located {
    let rest = rest.trim_start();
    let dll = if let Some(q) = rest.strip_prefix('"') {
        match q.find('"') {
            Some(end) => &q[..end],
            None => return Located::Unknown,
        }
    } else {
        match rest.find(',') {
            Some(i) => &rest[..i],
            None => return Located::Unknown,
        }
    };
    let dll = dll.trim();
    if dll.is_empty() || dll.starts_with(['/', '-']) {
        return Located::Unknown;
    }
    classify(p, dll, false)
}

/// Judge a command line: the program (or, for `rundll32`, its library) must be identifiable
/// and gone for the result to be [`Located::Missing`].
pub fn locate_command<P: WinProbe + ?Sized>(p: &P, raw: &str) -> Located {
    let Some(s) = prepare(p, raw) else {
        return Located::Unknown;
    };
    let s = s.trim();

    // rundll32 is judged by the library it loads.
    let (first, after_first) = if let Some(q) = s.strip_prefix('"') {
        match q.find('"') {
            Some(end) => (&q[..end], &q[end + 1..]),
            None => return Located::Unknown,
        }
    } else {
        match s.find(char::is_whitespace) {
            Some(i) => (&s[..i], &s[i..]),
            None => (s, ""),
        }
    };
    let b = basename_lower(first);
    if b == "rundll32" || b == "rundll32.exe" {
        return locate_rundll_target(p, after_first);
    }

    // Candidate program paths, in the order CreateProcess would try them.
    let mut cands: Vec<(String, bool)> = Vec::new(); // (path, ends in a known extension)
    let mut inner_looks_like_args = false;
    if let Some(q) = s.strip_prefix('"') {
        let end = q.find('"');
        let Some(end) = end else {
            return Located::Unknown;
        };
        let inner = q[..end].trim();
        if inner.is_empty() {
            return Located::Unknown;
        }
        inner_looks_like_args = inner
            .split_whitespace()
            .skip(1)
            .any(|t| t.starts_with(['/', '-']) && t.len() > 1);
        cands.push((inner.to_string(), has_known_ext(inner)));
    } else {
        let mut pos = 0;
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            // token [start, end)
            while i < bytes.len() && (bytes[i] as char).is_whitespace() {
                i += 1;
            }
            let start = i;
            while i < bytes.len() && !(bytes[i] as char).is_whitespace() {
                i += 1;
            }
            if start == i {
                break;
            }
            let token = &s[start..i];
            if token.contains('"') && pos > 0 {
                break; // arguments start here
            }
            cands.push((s[..i].to_string(), has_known_ext(token)));
            pos += 1;
            if pos > 12 {
                break; // no sane path has more than a dozen spaces
            }
        }
        // Keep the first token, plus every prefix that ends in a known extension.
        let first_only = cands.first().cloned();
        cands.retain(|(_, ext)| *ext);
        if let Some(f) = first_only {
            if !cands.iter().any(|(c, _)| *c == f.0) {
                cands.insert(0, f);
            }
        }
    }
    if cands.is_empty() {
        return Located::Unknown;
    }

    let mut verdicts: Vec<(Located, bool)> = Vec::new();
    for (c, ext) in &cands {
        let v = classify(p, c, true);
        if v == Located::Present {
            return Located::Present;
        }
        verdicts.push((v, *ext));
    }
    if verdicts.iter().any(|(v, _)| *v == Located::Unknown) {
        return Located::Unknown;
    }
    // Every spelling is missing. Report only when the program clearly has an executable
    // extension (something like `C:\dir\file.xyz` might be a document opened by a launcher).
    match verdicts.into_iter().find(|(_, ext)| *ext) {
        Some((Located::Missing(path), _)) if !inner_looks_like_args => Located::Missing(path),
        _ => Located::Unknown,
    }
}

/// Type library values end in an optional resource index: `C:\x\lib.dll\2`.
pub fn strip_resource_index(path: &str) -> &str {
    let t = path.trim();
    if let Some((head, tail)) = t.rsplit_once('\\') {
        if !tail.is_empty()
            && tail.bytes().all(|b| b.is_ascii_digit())
            && head
                .rsplit('\\')
                .next()
                .is_some_and(|seg| seg.contains('.'))
        {
            return head;
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::super::fake::FakeWin;
    use super::*;

    fn w() -> FakeWin {
        let w = FakeWin::new();
        w.file(r"C:\Windows\System32\notepad.exe")
            .file(r"C:\Windows\System32\shell32.dll")
            .file(r"C:\Windows\System32\rundll32.exe")
            .file(r"C:\Windows\System32\cmd.exe")
            .file(r"C:\Windows\System32\msiexec.exe")
            .file(r"C:\Windows\SysWOW64\only32.dll")
            .file(r"C:\Program Files\Acme App\acme.exe")
            .file(r"C:\Program Files (x86)\Old App\old.exe")
            .file(r"C:\Tools\tool.exe")
            .file(r"C:\Program Files\Two Words\one two.exe");
        w
    }

    fn missing(p: &str) -> Located {
        Located::Missing(p.to_string())
    }

    // ---------------- env / prefixes

    #[test]
    fn env_expansion() {
        let w = w();
        assert_eq!(expand_env(&w, r"%SystemRoot%\x"), r"C:\Windows\x");
        assert_eq!(expand_env(&w, r"%systemroot%\x"), r"C:\Windows\x");
        assert_eq!(expand_env(&w, "100%"), "100%");
        assert_eq!(expand_env(&w, r"%NOPE%\x"), r"%NOPE%\x");
        assert_eq!(expand_env(&w, "a%%b"), "a%%b");
        assert_eq!(
            expand_env(&w, r"%ProgramFiles(x86)%\a"),
            r"C:\Program Files (x86)\a"
        );
        // "%1 %SystemRoot%" : the first % cannot start a reference (whitespace inside)
        assert_eq!(expand_env(&w, r"%1 %SystemRoot%"), r"%1 C:\Windows");
    }

    #[test]
    fn nt_prefixes() {
        let w = w();
        assert_eq!(
            locate_command(&w, r"\SystemRoot\System32\notepad.exe"),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r"SystemRoot\System32\notepad.exe"),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r"\??\C:\Windows\System32\notepad.exe"),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r"\\?\C:\Windows\System32\gone.exe"),
            missing(r"C:\Windows\System32\gone.exe")
        );
        assert_eq!(
            locate_command(&w, r"\\?\UNC\srv\share\a.exe"),
            Located::Unknown
        );
        assert_eq!(
            locate_command(&w, r"\SystemRoot\System32\gone.exe"),
            missing(r"C:\Windows\System32\gone.exe")
        );
    }

    // ---------------- quoted / unquoted

    #[test]
    fn quoted_paths() {
        let w = w();
        assert_eq!(
            locate_command(&w, r#""C:\Program Files\Acme App\acme.exe" /silent"#),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r#""C:\Program Files\Acme App\gone.exe" /silent"#),
            missing(r"C:\Program Files\Acme App\gone.exe")
        );
        assert_eq!(
            locate_command(&w, r#""C:\Program Files\Acme App\acme.exe""#),
            Located::Present
        );
        // arguments containing quotes and unresolved parameters do not matter
        assert_eq!(
            locate_command(&w, r#""C:\Program Files\Acme App\acme.exe" "%1" --x="a b""#),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r#""C:\unterminated\a.exe"#),
            Located::Unknown
        );
        assert_eq!(locate_command(&w, r#"""  /x"#), Located::Unknown);
    }

    #[test]
    fn unquoted_paths_with_spaces_are_probed() {
        let w = w();
        assert_eq!(
            locate_command(&w, r"C:\Program Files\Acme App\acme.exe /S /D=C:\x"),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r"C:\Program Files\Acme App\gone.exe /S"),
            missing(r"C:\Program Files\Acme App\gone.exe")
        );
        // the file name itself contains a space
        assert_eq!(
            locate_command(&w, r"C:\Program Files\Two Words\one two.exe -q"),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r"C:\Program Files\Two Words\one two.exe"),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r"C:\Tools\tool.exe --dir C:\Some Dir\x.exe"),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r"C:\Tools\gone.exe --dir C:\Some Dir\x.exe"),
            missing(r"C:\Tools\gone.exe")
        );
    }

    #[test]
    fn extension_is_a_token_boundary() {
        let w = w();
        w.file(r"C:\Tools\my.exe.config\run.exe");
        assert_eq!(
            locate_command(&w, r"C:\Tools\my.exe.config\run.exe /q"),
            Located::Present
        );
    }

    #[test]
    fn no_extension_appends_exe_like_createprocess() {
        let w = w();
        assert_eq!(locate_command(&w, r"C:\Tools\tool"), Located::Present);
        assert_eq!(locate_command(&w, r"C:\Tools\tool -x"), Located::Present);
        // missing but without an executable extension: ambiguous -> not reported
        assert_eq!(locate_command(&w, r"C:\Tools\nothere"), Located::Unknown);
        assert_eq!(locate_command(&w, r"C:\Tools\file.xyz"), Located::Unknown);
    }

    #[test]
    fn quoted_path_with_switches_inside_the_quotes_is_ambiguous() {
        let w = w();
        assert_eq!(
            locate_command(&w, r#""C:\Tools\gone.exe /s""#),
            Located::Unknown
        );
        assert_eq!(
            locate_command(&w, r#""C:\Tools\tool.exe /s""#),
            Located::Unknown
        );
    }

    // ---------------- redirection

    #[test]
    fn wow64_redirection_counts_as_present() {
        let w = w();
        assert_eq!(
            locate_command(&w, r"C:\Windows\System32\only32.dll"),
            Located::Present
        );
        assert_eq!(
            locate_path(&w, r"%SystemRoot%\system32\only32.dll"),
            Located::Present
        );
        assert_eq!(
            locate_path(&w, r"C:\WINDOWS\SYSTEM32\gone32.dll"),
            missing(r"C:\WINDOWS\SYSTEM32\gone32.dll")
        );
        // Program Files <-> Program Files (x86)
        assert_eq!(
            locate_command(&w, r"C:\Program Files\Old App\old.exe"),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r"C:\Program Files (x86)\Acme App\acme.exe"),
            Located::Present
        );
    }

    #[test]
    fn case_and_slashes() {
        let w = w();
        assert_eq!(locate_command(&w, r"c:/tools/TOOL.EXE"), Located::Present);
        assert_eq!(
            locate_command(&w, r"c:/tools/gone.exe"),
            missing(r"c:\tools\gone.exe")
        );
    }

    // ---------------- environment

    #[test]
    fn env_var_paths() {
        let w = w();
        assert_eq!(
            locate_command(&w, r#""%ProgramFiles%\Acme App\acme.exe" -x"#),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r#"%ProgramFiles%\Acme App\gone.exe -x"#),
            missing(r"C:\Program Files\Acme App\gone.exe")
        );
        assert_eq!(locate_command(&w, r"%UNKNOWN%\a.exe"), Located::Unknown);
        assert_eq!(locate_path(&w, r"%UNKNOWN%\a.dll"), Located::Unknown);
        assert_eq!(locate_command(&w, "%1"), Located::Unknown);
        assert_eq!(locate_command(&w, r#""%1" /x"#), Located::Unknown);
    }

    // ---------------- rundll32

    #[test]
    fn rundll32_follows_the_library() {
        let w = w();
        w.file(r"C:\Program Files\Acme App\acme.dll");
        assert_eq!(
            locate_command(&w, "rundll32.exe shell32.dll,Control_RunDLL x.cpl"),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r#"rundll32 "C:\Program Files\Acme App\acme.dll",Entry"#),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r#"rundll32 "C:\Program Files\Acme App\gone.dll",Entry"#),
            missing(r"C:\Program Files\Acme App\gone.dll")
        );
        // unquoted path with spaces: the library ends at the comma
        assert_eq!(
            locate_command(
                &w,
                r"rundll32.exe C:\Program Files\Acme App\gone.dll,Entry 1"
            ),
            missing(r"C:\Program Files\Acme App\gone.dll")
        );
        assert_eq!(
            locate_command(
                &w,
                r"C:\Windows\System32\rundll32.exe C:\Program Files\Acme App\acme.dll,Entry"
            ),
            Located::Present
        );
        // a bare library that cannot be found is ambiguous
        assert_eq!(
            locate_command(&w, "rundll32.exe nowhere.dll,Entry"),
            Located::Unknown
        );
        assert_eq!(locate_command(&w, "rundll32.exe"), Located::Unknown);
        assert_eq!(locate_command(&w, "rundll32.exe /s"), Located::Unknown);
        assert_eq!(
            locate_command(&w, "rundll32.exe %SystemRoot%\\system32\\shell32.dll,Entry"),
            Located::Present
        );
    }

    // ---------------- bare names / launchers

    #[test]
    fn bare_names_are_present_or_unknown_never_missing() {
        let w = w();
        assert_eq!(locate_command(&w, "notepad.exe"), Located::Present);
        assert_eq!(locate_command(&w, "notepad"), Located::Present);
        assert_eq!(locate_command(&w, "tool.exe"), Located::Present); // via PATH
        assert_eq!(locate_command(&w, "nothere.exe"), Located::Unknown);
        assert_eq!(locate_path(&w, "shell32.dll"), Located::Present);
        assert_eq!(locate_path(&w, "nothere.dll"), Located::Unknown);
    }

    #[test]
    fn launchers_are_judged_by_the_launcher() {
        let w = w();
        assert_eq!(
            locate_command(&w, r"cmd.exe /c C:\gone\thing.bat"),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, "MsiExec.exe /X{90160000-008C-0000-0000-0000000FF1CE}"),
            Located::Present
        );
        assert_eq!(
            locate_command(&w, r#"C:\Windows\System32\cmd.exe /c "C:\gone\x.bat""#),
            Located::Present
        );
    }

    // ---------------- drives, UNC, permissions

    #[test]
    fn removable_network_and_missing_drives_are_unknown() {
        let w = w();
        w.removable_drive('E');
        assert_eq!(locate_command(&w, r"E:\usb\app.exe"), Located::Unknown);
        assert_eq!(locate_path(&w, r"E:\usb\app.dll"), Located::Unknown);
        // Z: does not exist at all (unplugged / unmapped)
        assert_eq!(locate_command(&w, r"Z:\net\app.exe"), Located::Unknown);
        // a fixed second drive is fine
        w.fixed_drive('D');
        assert_eq!(
            locate_command(&w, r"D:\Games\g.exe"),
            missing(r"D:\Games\g.exe")
        );
        w.file(r"D:\Games\g.exe");
        assert_eq!(locate_command(&w, r"D:\Games\g.exe"), Located::Present);
    }

    #[test]
    fn unc_relative_and_odd_paths_are_unknown() {
        let w = w();
        for s in [
            r"\\server\share\a.exe",
            r"..\a.exe",
            r".\a.exe",
            r"\Windows\a.exe",
            r"C:a.exe",
            r"C:",
            r"C:\a\..\b.exe",
            r"C:\a*.exe",
            r"C:\a?.exe",
            r"C:\a|b.exe",
            "",
            "   ",
            "-x",
            "/S",
            "@C:\\x.exe",
        ] {
            assert_eq!(locate_command(&w, s), Located::Unknown, "{s:?}");
        }
    }

    #[test]
    fn unreadable_locations_are_unknown() {
        let w = w();
        w.fixed_drive('D');
        w.unreadable(r"D:\Private\a.exe");
        assert_eq!(locate_command(&w, r"D:\Private\a.exe"), Located::Unknown);
        // even one unreadable alternate blocks "missing"
        w.unreadable(r"C:\Windows\SysWOW64\x.dll");
        assert_eq!(
            locate_path(&w, r"C:\Windows\System32\x.dll"),
            Located::Unknown
        );
    }

    #[test]
    fn locked_volume_is_unknown() {
        let w = w();
        w.fixed_drive('D');
        w.unreadable(r"D:\");
        assert_eq!(locate_command(&w, r"D:\a\b.exe"), Located::Unknown);
    }

    #[test]
    fn windows_relative_paths_only_present_or_unknown() {
        let w = w();
        assert_eq!(
            locate_command(&w, r"System32\notepad.exe -k x"),
            Located::Present
        );
        assert_eq!(locate_command(&w, r"System32\gone.exe"), Located::Unknown);
    }

    // ---------------- plain paths

    #[test]
    fn plain_paths() {
        let w = w();
        assert_eq!(locate_path(&w, r"C:\Tools\tool.exe"), Located::Present);
        assert_eq!(locate_path(&w, r#""C:\Tools\tool.exe""#), Located::Present);
        assert_eq!(locate_path(&w, r"  C:\Tools\tool.exe  "), Located::Present);
        assert_eq!(
            locate_path(&w, r"C:\Tools\gone.dll"),
            missing(r"C:\Tools\gone.dll")
        );
        assert_eq!(locate_path(&w, r"C:\Tools"), Located::Present); // directories too
        assert_eq!(locate_path(&w, r"C:\Tools\"), Located::Present);
        assert_eq!(locate_path(&w, r"C:\a.dll;C:\b.dll"), Located::Unknown);
        assert_eq!(locate_path(&w, ""), Located::Unknown);
    }

    #[test]
    fn resource_index_suffix() {
        assert_eq!(strip_resource_index(r"C:\x\lib.dll\2"), r"C:\x\lib.dll");
        assert_eq!(strip_resource_index(r"C:\x\lib.tlb"), r"C:\x\lib.tlb");
        assert_eq!(strip_resource_index(r"C:\x\12"), r"C:\x\12");
        assert_eq!(strip_resource_index(r" C:\x.dll\10 "), r"C:\x.dll");
    }

    #[test]
    fn extension_detection() {
        assert!(has_known_ext(r"C:\a\b.EXE"));
        assert!(has_known_ext("b.dll"));
        assert!(!has_known_ext(r"C:\a.dir\b"));
        assert!(!has_known_ext("b.txt"));
        assert!(!has_known_ext(".exe")); // a dotfile is not a program
    }
}
