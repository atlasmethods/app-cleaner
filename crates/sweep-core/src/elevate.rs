//! Privilege elevation for the few operations that need administrator rights
//! (package managers, driver installs, machine-wide uninstallers).
//!
//! - [`is_elevated`]: is the current process already root / an elevated administrator?
//! - [`run_privileged`]: run a program with administrator rights through
//!   [`Ctx::runner`](crate::ctx::Ctx), wrapping it in the platform's own authorization
//!   dialog when we are not elevated:
//!   - Linux: `pkexec <program> <args...>` (polkit)
//!   - macOS: `osascript -e 'do shell script "<escaped command>" with administrator privileges'`
//!   - Windows: `powershell ... Start-Process -Verb RunAs -Wait -PassThru` around
//!     `cmd.exe /c` so the elevated program's output can be captured through a temp file.
//!
//! The wrapper is chosen from `ctx.env.os` (not `cfg!`), so all three variants are
//! unit-tested on any host with a `MockRunner`. The program's own exit status is returned
//! in the [`CmdOutput`]; only "no way to elevate" and "the user declined the dialog" are
//! errors (`PermissionDenied`).

use std::cell::Cell;

use crate::ctx::{Ctx, Os};
use crate::error::{ApiError, Result};
use crate::fsutil::random_id;
use crate::runner::CmdOutput;

pub const MSG_NEED_ADMIN: &str =
    "Administrator rights are required; install polkit or run ClearSweep as root";
pub const MSG_CANCELLED: &str = "Authorization was cancelled";

thread_local! {
    static OVERRIDE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Run `f` with [`is_elevated`] forced to `elevated` on this thread (tests only).
#[cfg(any(test, feature = "testutil"))]
pub fn with_elevation<T>(elevated: bool, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<bool>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OVERRIDE.with(|o| o.set(self.0));
        }
    }
    let _restore = Restore(OVERRIDE.with(|o| o.replace(Some(elevated))));
    f()
}

/// True when the process runs as root (unix) or with an elevated token (Windows).
pub fn is_elevated() -> bool {
    if let Some(v) = OVERRIDE.with(|o| o.get()) {
        return v;
    }
    real_is_elevated()
}

#[cfg(unix)]
fn real_is_elevated() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(windows)]
fn real_is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY: plain Win32 calls on our own process token; the handle is closed below and
    // the output buffer is a correctly sized TOKEN_ELEVATION.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(any(unix, windows)))]
fn real_is_elevated() -> bool {
    false
}

/// Run `program args...` with administrator rights (see the module docs).
pub fn run_privileged(ctx: &Ctx, program: &str, args: &[&str]) -> Result<CmdOutput> {
    if is_elevated() {
        return ctx.runner.run(program, args);
    }
    match ctx.env.os {
        Os::Linux => run_pkexec(ctx, program, args),
        Os::MacOs => run_osascript(ctx, program, args),
        Os::Windows => run_windows_runas(ctx, program, args),
    }
}

// ---------------------------------------------------------------- Linux

fn run_pkexec(ctx: &Ctx, program: &str, args: &[&str]) -> Result<CmdOutput> {
    if ctx.runner.which("pkexec").is_none() {
        return Err(ApiError::permission_denied(MSG_NEED_ADMIN));
    }
    let mut full: Vec<&str> = Vec::with_capacity(args.len() + 1);
    full.push(program);
    full.extend_from_slice(args);
    let out = ctx.runner.run("pkexec", &full)?;
    match out.status {
        126 => Err(ApiError::permission_denied(MSG_CANCELLED)),
        127 => {
            let e = out.stderr.to_lowercase();
            if e.contains("no such file") || e.contains("not found") {
                Err(ApiError::not_found(format!("command not found: {program}")))
            } else {
                Err(ApiError::permission_denied(MSG_CANCELLED))
            }
        }
        _ => Ok(out),
    }
}

// ---------------------------------------------------------------- macOS

/// POSIX shell quoting: bare when safe, otherwise single-quoted with `'` -> `'\''`.
pub fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:=@+,".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Escape text for use inside an AppleScript double-quoted string literal.
pub fn applescript_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// The AppleScript run by `osascript -e`, for `program args...`.
pub fn macos_admin_script(program: &str, args: &[&str]) -> String {
    let mut cmd = shell_quote(program);
    for a in args {
        cmd.push(' ');
        cmd.push_str(&shell_quote(a));
    }
    cmd.push_str(" 2>&1");
    format!(
        "do shell script \"{}\" with administrator privileges",
        applescript_escape(&cmd)
    )
}

/// `execution error: ... (3)` -> `Some(3)`.
fn trailing_paren_number(s: &str) -> Option<i32> {
    let t = s.trim_end();
    let t = t.strip_suffix(')')?;
    let open = t.rfind('(')?;
    t[open + 1..].trim().parse().ok()
}

fn run_osascript(ctx: &Ctx, program: &str, args: &[&str]) -> Result<CmdOutput> {
    let script = macos_admin_script(program, args);
    let out = ctx.runner.run("osascript", &["-e", &script])?;
    if out.status == 0 {
        return Ok(out);
    }
    let err = out.stderr.trim();
    let code = trailing_paren_number(err);
    if code == Some(-128) || err.contains("User canceled") || err.contains("User cancelled") {
        return Err(ApiError::permission_denied(MSG_CANCELLED));
    }
    match code {
        // `do shell script` failed with the command's own exit status; the message before it
        // is the command's (merged) output.
        Some(n) if n > 0 => {
            let body = err
                .rsplit_once('(')
                .map(|(b, _)| b.trim_end())
                .unwrap_or(err);
            let body = body
                .split_once("execution error:")
                .map(|(_, b)| b.trim())
                .unwrap_or(body);
            Ok(CmdOutput {
                status: n,
                stdout: String::new(),
                stderr: body.to_string(),
            })
        }
        _ => Err(ApiError::permission_denied(format!(
            "Could not obtain administrator rights: {err}"
        ))),
    }
}

// ---------------------------------------------------------------- Windows

/// Marker written to stderr by the PowerShell wrapper when the UAC prompt is declined.
pub const WIN_CANCEL_MARKER: &str = "CLEARSWEEP_ELEVATION_CANCELLED";

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Characters that cannot be quoted safely inside a `cmd.exe /c "..."` line.
fn cmd_unsafe(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c, '"' | '%' | '\n' | '\r' | '\0'))
}

/// The `cmd.exe` command line: every token quoted, output redirected to `out`.
pub fn windows_cmd_line(program: &str, args: &[&str], out: &str) -> Result<String> {
    for t in std::iter::once(&program).chain(args.iter()).chain([&out]) {
        if cmd_unsafe(t) {
            return Err(ApiError::invalid_params(format!(
                "cannot run `{t}` elevated: contains a quote, percent sign or line break"
            )));
        }
    }
    let mut line = format!("/d /s /c \"\"{program}\"");
    for a in args {
        line.push_str(&format!(" \"{a}\""));
    }
    line.push_str(&format!(" > \"{out}\" 2>&1\""));
    Ok(line)
}

/// The PowerShell script that elevates `cmd.exe` with the given command line.
pub fn windows_runas_script(cmd_line: &str, out: &str) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $out = {out}\n\
         try {{\n\
         \x20 $p = Start-Process -FilePath 'cmd.exe' -ArgumentList {args} -Verb RunAs -Wait -PassThru -WindowStyle Hidden\n\
         }} catch {{\n\
         \x20 if ($_.Exception.NativeErrorCode -eq 1223) {{ [Console]::Error.WriteLine('{marker}'); exit 1223 }}\n\
         \x20 [Console]::Error.WriteLine($_.Exception.Message); exit 1\n\
         }}\n\
         if (Test-Path -LiteralPath $out) {{ Get-Content -Raw -LiteralPath $out; Remove-Item -Force -LiteralPath $out }}\n\
         exit $p.ExitCode\n",
        out = ps_quote(out),
        args = ps_quote(cmd_line),
        marker = WIN_CANCEL_MARKER,
    )
}

/// Standard base64.
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// `-EncodedCommand` payload: base64 of the UTF-16LE script.
pub fn powershell_encode(script: &str) -> String {
    let bytes: Vec<u8> = script
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    base64(&bytes)
}

/// Inverse of [`powershell_encode`] (used to inspect the scripts we generate in tests).
pub fn powershell_decode(b64: &str) -> Option<String> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let vals: Vec<u32> = b64
        .bytes()
        .filter(|b| *b != b'=')
        .map(|b| T.iter().position(|x| *x == b).map(|p| p as u32))
        .collect::<Option<_>>()?;
    let mut bytes = Vec::new();
    for ch in vals.chunks(4) {
        let mut n = 0u32;
        for (k, v) in ch.iter().enumerate() {
            n |= v << (18 - 6 * k);
        }
        bytes.push((n >> 16) as u8);
        if ch.len() > 2 {
            bytes.push((n >> 8) as u8);
        }
        if ch.len() > 3 {
            bytes.push(n as u8);
        }
    }
    let u16s: Vec<u16> = bytes
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&u16s).ok()
}

fn run_windows_runas(ctx: &Ctx, program: &str, args: &[&str]) -> Result<CmdOutput> {
    let out_path = ctx
        .env
        .temp_dir
        .join(format!("clearsweep-elev-{}.txt", random_id()));
    let out_str = out_path.to_string_lossy().into_owned();
    let cmd_line = windows_cmd_line(program, args, &out_str)?;
    let script = windows_runas_script(&cmd_line, &out_str);
    let encoded = powershell_encode(&script);
    let out = ctx.runner.run(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ],
    )?;
    if out.status == 1223 && out.stderr.contains(WIN_CANCEL_MARKER) {
        return Err(ApiError::permission_denied(MSG_CANCELLED));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use crate::runner::MockRunner;
    use std::path::Path;

    fn ctx_for(os: Os, m: &MockRunner) -> Ctx {
        let mut c = Ctx::test(Path::new("/tmp/elev-test"), m.clone());
        c.env.os = os;
        c
    }

    #[test]
    fn elevated_runs_directly_on_every_os() {
        for os in [Os::Linux, Os::MacOs, Os::Windows] {
            let m = MockRunner::new();
            m.on("apt-get", &["update"], CmdOutput::ok("done"));
            let c = ctx_for(os, &m);
            let out = with_elevation(true, || run_privileged(&c, "apt-get", &["update"])).unwrap();
            assert_eq!(out.stdout, "done");
            assert_eq!(
                m.calls(),
                vec![("apt-get".to_string(), vec!["update".to_string()])]
            );
        }
    }

    #[test]
    fn real_is_elevated_matches_euid_on_unix() {
        #[cfg(unix)]
        assert_eq!(real_is_elevated(), unsafe { libc::geteuid() } == 0);
        // The override is restored after with_elevation.
        let before = is_elevated();
        with_elevation(!before, || assert_eq!(is_elevated(), !before));
        assert_eq!(is_elevated(), before);
    }

    #[test]
    fn linux_wraps_in_pkexec() {
        let m = MockRunner::new();
        m.on(
            "pkexec",
            &["apt-get", "remove", "-y", "foo"],
            CmdOutput::ok("removed"),
        );
        let c = ctx_for(Os::Linux, &m);
        let out = with_elevation(false, || {
            run_privileged(&c, "apt-get", &["remove", "-y", "foo"])
        })
        .unwrap();
        assert_eq!(out.stdout, "removed");
        assert_eq!(m.calls()[0].0, "pkexec");
    }

    #[test]
    fn linux_without_pkexec_is_permission_denied_with_clear_message() {
        let m = MockRunner::new();
        let c = ctx_for(Os::Linux, &m);
        let e = with_elevation(false, || run_privileged(&c, "apt-get", &["update"])).unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied);
        assert_eq!(
            e.message,
            "Administrator rights are required; install polkit or run ClearSweep as root"
        );
        assert!(m.calls().is_empty());
    }

    #[test]
    fn pkexec_dismissed_dialog_is_cancelled() {
        for code in [126, 127] {
            let m = MockRunner::new();
            m.on(
                "pkexec",
                &["apt-get", "update"],
                CmdOutput::failed(
                    code,
                    "Error executing command as another user: Not authorized",
                ),
            );
            let c = ctx_for(Os::Linux, &m);
            let e =
                with_elevation(false, || run_privileged(&c, "apt-get", &["update"])).unwrap_err();
            assert_eq!(e.code, ErrorCode::PermissionDenied, "{code}");
            assert_eq!(e.message, "Authorization was cancelled");
        }
    }

    #[test]
    fn pkexec_127_for_missing_program_is_not_found() {
        let m = MockRunner::new();
        m.on(
            "pkexec",
            &["nope"],
            CmdOutput::failed(127, "pkexec: nope: No such file or directory"),
        );
        let c = ctx_for(Os::Linux, &m);
        let e = with_elevation(false, || run_privileged(&c, "nope", &[])).unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
    }

    #[test]
    fn pkexec_passes_through_the_programs_own_failure() {
        let m = MockRunner::new();
        m.on(
            "pkexec",
            &["apt-get", "install", "x"],
            CmdOutput::failed(100, "E: Unable to locate package x"),
        );
        let c = ctx_for(Os::Linux, &m);
        let out =
            with_elevation(false, || run_privileged(&c, "apt-get", &["install", "x"])).unwrap();
        assert_eq!(out.status, 100);
    }

    #[test]
    fn shell_quote_cases() {
        assert_eq!(shell_quote("softwareupdate"), "softwareupdate");
        assert_eq!(shell_quote("-i"), "-i");
        assert_eq!(shell_quote("/usr/sbin/x"), "/usr/sbin/x");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("$(rm -rf /)"), "'$(rm -rf /)'");
        assert_eq!(shell_quote("a;b"), "'a;b'");
        assert_eq!(shell_quote("`x`"), "'`x`'");
    }

    #[test]
    fn applescript_escape_cases() {
        assert_eq!(applescript_escape(r#"a"b"#), r#"a\"b"#);
        assert_eq!(applescript_escape(r"a\b"), r"a\\b");
        assert_eq!(applescript_escape("a\nb"), "a\\nb");
        // Backslash is escaped before the quote it precedes is, so `\"` cannot break out.
        assert_eq!(applescript_escape(r#"\""#), r#"\\\""#);
    }

    #[test]
    fn macos_script_is_quoted_then_escaped() {
        let s = macos_admin_script("softwareupdate", &["-i", "macOS Sonoma 14.5-23F79"]);
        assert_eq!(
            s,
            "do shell script \"softwareupdate -i 'macOS Sonoma 14.5-23F79' 2>&1\" with administrator privileges"
        );
        // A hostile label cannot terminate either quoting layer.
        let s = macos_admin_script("softwareupdate", &["-i", "x'; rm -rf ~; echo \"y"]);
        assert_eq!(
            s,
            "do shell script \"softwareupdate -i 'x'\\\\''; rm -rf ~; echo \\\"y' 2>&1\" with administrator privileges"
        );
    }

    #[test]
    fn macos_wraps_in_osascript_and_maps_results() {
        let script = macos_admin_script("softwareupdate", &["-l"]);
        let m = MockRunner::new();
        m.on(
            "osascript",
            &["-e", &script],
            CmdOutput::ok("Software Update Tool\n"),
        );
        let c = ctx_for(Os::MacOs, &m);
        let out = with_elevation(false, || run_privileged(&c, "softwareupdate", &["-l"])).unwrap();
        assert!(out.stdout.contains("Software Update Tool"));

        let m = MockRunner::new();
        m.on(
            "osascript",
            &["-e", &script],
            CmdOutput::failed(1, "0:56: execution error: User canceled. (-128)\n"),
        );
        let c = ctx_for(Os::MacOs, &m);
        let e =
            with_elevation(false, || run_privileged(&c, "softwareupdate", &["-l"])).unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied);
        assert_eq!(e.message, "Authorization was cancelled");

        let m = MockRunner::new();
        m.on(
            "osascript",
            &["-e", &script],
            CmdOutput::failed(
                1,
                "35:71: execution error: softwareupdate: No such update (2)\n",
            ),
        );
        let c = ctx_for(Os::MacOs, &m);
        let out = with_elevation(false, || run_privileged(&c, "softwareupdate", &["-l"])).unwrap();
        assert_eq!(out.status, 2);
        assert_eq!(out.stderr, "softwareupdate: No such update");
    }

    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // PowerShell's own documented example: "dir" -> ZABpAHIA
        assert_eq!(powershell_encode("dir"), "ZABpAHIA");
    }

    #[test]
    fn windows_cmd_line_quotes_every_token_and_rejects_unquotable() {
        let l = windows_cmd_line(
            r"C:\Program Files\Foo\unins000.exe",
            &["/SILENT", "/D=C:\\x y"],
            r"C:\Temp\o.txt",
        )
        .unwrap();
        assert_eq!(
            l,
            r#"/d /s /c ""C:\Program Files\Foo\unins000.exe" "/SILENT" "/D=C:\x y" > "C:\Temp\o.txt" 2>&1""#
        );
        assert_eq!(
            windows_cmd_line("a", &["b\"c"], "o").unwrap_err().code,
            ErrorCode::InvalidParams
        );
        assert_eq!(
            windows_cmd_line("a", &["%PATH%"], "o").unwrap_err().code,
            ErrorCode::InvalidParams
        );
    }

    #[test]
    fn windows_script_escapes_single_quotes() {
        let s = windows_runas_script("/c \"it's\"", r"C:\Users\O'Neil\o.txt");
        assert!(s.contains("'C:\\Users\\O''Neil\\o.txt'"), "{s}");
        assert!(s.contains("-ArgumentList '/c \"it''s\"'"), "{s}");
        assert!(s.contains("-Verb RunAs -Wait -PassThru"));
        assert!(s.contains(WIN_CANCEL_MARKER));
    }

    fn decode_encoded_command(args: &[String]) -> String {
        let i = args.iter().position(|a| a == "-EncodedCommand").unwrap();
        powershell_decode(&args[i + 1]).unwrap()
    }

    #[test]
    fn windows_wraps_in_powershell_runas() {
        let m = MockRunner::new();
        // The temp file name is random, so script the reply for any arguments.
        m.on_any_args("powershell", CmdOutput::ok("Exported 3 drivers\n"));
        let c = ctx_for(Os::Windows, &m);
        let out = with_elevation(false, || {
            run_privileged(&c, "pnputil", &["/export-driver", "*", r"C:\b\drivers"])
        })
        .unwrap();
        assert_eq!(out.stdout, "Exported 3 drivers\n");
        let calls = m.calls();
        assert_eq!(calls[0].0, "powershell");
        assert_eq!(
            &calls[0].1[..4],
            [
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass"
            ]
        );
        let script = decode_encoded_command(&calls[0].1);
        assert!(
            script.contains("Start-Process -FilePath 'cmd.exe'"),
            "{script}"
        );
        assert!(script.contains("-Verb RunAs -Wait -PassThru"));
        assert!(
            script.contains(r#""pnputil" "/export-driver" "*" "C:\b\drivers""#),
            "{script}"
        );
        assert!(script.contains("2>&1"));
    }

    #[test]
    fn windows_declined_uac_is_cancelled() {
        let m = MockRunner::new();
        m.on_any_args(
            "powershell",
            CmdOutput::failed(1223, format!("{WIN_CANCEL_MARKER}\r\n")),
        );
        let c = ctx_for(Os::Windows, &m);
        let e = with_elevation(false, || run_privileged(&c, "pnputil", &["/enum-drivers"]))
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied);
        assert_eq!(e.message, "Authorization was cancelled");
        // A program that itself exits 1223 (no marker) is passed through.
        let m = MockRunner::new();
        m.on_any_args("powershell", CmdOutput::failed(1223, "x"));
        let c = ctx_for(Os::Windows, &m);
        let out =
            with_elevation(false, || run_privileged(&c, "pnputil", &["/enum-drivers"])).unwrap();
        assert_eq!(out.status, 1223);
    }

    #[test]
    fn windows_unquotable_args_are_rejected_before_running() {
        let m = MockRunner::new();
        let c = ctx_for(Os::Windows, &m);
        let e = with_elevation(false, || run_privileged(&c, "x.exe", &["a\"b"])).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidParams);
        assert!(m.calls().is_empty());
    }
}
