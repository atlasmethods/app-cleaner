//! Small helpers shared by the uninstall / software updater / driver updater features:
//! package-name validation, tool-output cleanup and size / date parsing.

use std::path::Path;
use time::OffsetDateTime;
use walkdir::WalkDir;

use crate::runner::CmdOutput;

/// Package / application ids that are safe to hand to a package manager as an argument:
/// no leading `-` (option injection), no whitespace or shell/path syntax.
pub fn valid_pkg_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    s.len() <= 255
        && (first.is_ascii_alphanumeric() || first == '_')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._+-:@~".contains(c))
}

/// Remove a trailing `<email>` from a maintainer string: `Foo Bar <a@b.c>` -> `Foo Bar`.
pub fn strip_email(s: &str) -> String {
    let t = s.trim();
    match t.find('<') {
        Some(i) if t.ends_with('>') => t[..i].trim().to_string(),
        _ => t.to_string(),
    }
}

/// Parse `1.5 MB`, `12 kB`, `3.2 MiB`, `250MB` (also with a no-break space).
/// `SI` units (kB, MB, GB) are 1000-based, `KiB`/`MiB`/`GiB` 1024-based.
pub fn parse_human_size(s: &str) -> Option<u64> {
    let t = s.trim();
    let split = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let num: f64 = num.replace(',', ".").parse().ok()?;
    let unit = unit.trim().to_ascii_lowercase();
    let mult: f64 = match unit.as_str() {
        "" | "b" | "bytes" => 1.0,
        "kb" => 1000.0,
        "mb" => 1000.0 * 1000.0,
        "gb" => 1000.0 * 1000.0 * 1000.0,
        "tb" => 1e12,
        "k" | "kib" => 1024.0,
        "m" | "mib" => 1024.0 * 1024.0,
        "g" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "tib" => 1024f64.powi(4),
        _ => return None,
    };
    let v = num * mult;
    (v.is_finite() && v >= 0.0).then_some(v as u64)
}

/// `1536` -> `1.5 KB` (for messages).
pub fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// `1700000000` -> `2023-11-14`.
pub fn date_from_unix(secs: i64) -> Option<String> {
    let d = OffsetDateTime::from_unix_timestamp(secs).ok()?.date();
    Some(format!(
        "{:04}-{:02}-{:02}",
        d.year(),
        u8::from(d.month()),
        d.day()
    ))
}

/// `20240131` -> `2024-01-31` (Windows `InstallDate`); anything else -> `None`.
pub fn date_from_yyyymmdd(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
        let (y, m, d) = (&s[..4], &s[4..6], &s[6..]);
        let (mi, di): (u32, u32) = (m.parse().ok()?, d.parse().ok()?);
        if (1..=12).contains(&mi) && (1..=31).contains(&di) && y != "0000" {
            return Some(format!("{y}-{m}-{d}"));
        }
    }
    None
}

/// Total size in bytes of a file or directory tree (symlinks are not followed).
pub fn path_size(p: &Path) -> u64 {
    WalkDir::new(p)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// A short human-readable summary of a finished command: the last few meaningful lines
/// of stderr (or stdout when stderr is empty), capped in length.
pub fn summarize(out: &CmdOutput) -> String {
    let src = if out.stderr.trim().is_empty() {
        &out.stdout
    } else {
        &out.stderr
    };
    let lines: Vec<&str> = src
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    let tail = &lines[lines.len().saturating_sub(3)..];
    let mut s = tail.join(" | ");
    if s.chars().count() > 300 {
        s = s.chars().take(300).collect::<String>() + "...";
    }
    s
}

/// Split at `\n`, and for each line keep only what follows the last `\r` (progress
/// spinners overwrite themselves with carriage returns). Strips a UTF-8 BOM.
pub fn clean_terminal_lines(s: &str) -> Vec<String> {
    let s = s.strip_prefix('\u{feff}').unwrap_or(s);
    s.split('\n')
        .map(|l| {
            let l = l.trim_end_matches('\r');
            l.rsplit('\r').next().unwrap_or(l).to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkg_names() {
        for ok in [
            "firefox",
            "libc6",
            "g++",
            "org.mozilla.firefox",
            "Mozilla.Firefox",
            "python3.12",
            "lib32-foo",
            "foo:amd64",
            "a_b",
        ] {
            assert!(valid_pkg_name(ok), "{ok}");
        }
        for bad in [
            "", "-y", "--purge", "a b", "a;b", "a/b", "$(x)", "a\nb", "`x`", "..", ".hidden",
        ] {
            assert!(!valid_pkg_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn email_stripped() {
        assert_eq!(
            strip_email("Ubuntu Developers <ubuntu-devel@lists.ubuntu.com>"),
            "Ubuntu Developers"
        );
        assert_eq!(strip_email("plain"), "plain");
        assert_eq!(strip_email("<only@mail>"), "");
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_human_size("1.5 MB"), Some(1_500_000));
        assert_eq!(parse_human_size("12\u{a0}kB"), Some(12_000));
        assert_eq!(parse_human_size("3 MiB"), Some(3 * 1024 * 1024));
        assert_eq!(parse_human_size("250MB"), Some(250_000_000));
        assert_eq!(parse_human_size("1,5 GB"), Some(1_500_000_000));
        assert_eq!(parse_human_size("512"), Some(512));
        assert_eq!(parse_human_size("?"), None);
        assert_eq!(parse_human_size("12 parsecs"), None);
        assert_eq!(parse_human_size(""), None);
    }

    #[test]
    fn dates() {
        assert_eq!(date_from_unix(1_700_000_000).as_deref(), Some("2023-11-14"));
        assert_eq!(
            date_from_yyyymmdd("20240131").as_deref(),
            Some("2024-01-31")
        );
        assert_eq!(date_from_yyyymmdd("2024-01-31"), None);
        assert_eq!(date_from_yyyymmdd("20241301"), None);
        assert_eq!(date_from_yyyymmdd(""), None);
    }

    #[test]
    fn summarize_takes_tail() {
        let o = CmdOutput::failed(100, "a\n\nb\nc\nd\n");
        assert_eq!(summarize(&o), "b | c | d");
        assert_eq!(summarize(&CmdOutput::ok("only out")), "only out");
        assert_eq!(summarize(&CmdOutput::ok("")), "");
    }

    #[test]
    fn terminal_lines_drop_spinner_and_bom() {
        let v = clean_terminal_lines("\u{feff}   - \r   \\ \r   | \rName  Id\r\n----\nrow\n");
        assert_eq!(v, vec!["Name  Id", "----", "row", ""]);
    }
}
