//! Parsers for the "what can be updated" output of every supported package manager.
//! Pure functions over captured tool output, so all platforms are tested on any host.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

use crate::pkgutil::{clean_terminal_lines, valid_pkg_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum USource {
    #[serde(rename = "apt")]
    Apt,
    #[serde(rename = "dnf")]
    Dnf,
    #[serde(rename = "pacman")]
    Pacman,
    #[serde(rename = "flatpak")]
    Flatpak,
    #[serde(rename = "snap")]
    Snap,
    #[serde(rename = "winget")]
    Winget,
    #[serde(rename = "brew")]
    Brew,
    #[serde(rename = "brew-cask")]
    BrewCask,
    #[serde(rename = "macos")]
    Macos,
}

impl USource {
    pub fn prefix(self) -> &'static str {
        match self {
            USource::Apt => "apt",
            USource::Dnf => "dnf",
            USource::Pacman => "pacman",
            USource::Flatpak => "flatpak",
            USource::Snap => "snap",
            USource::Winget => "winget",
            USource::Brew => "brew",
            USource::BrewCask => "brew-cask",
            USource::Macos => "macos",
        }
    }
}

/// One available update. `key` is the package manager's own identifier (package name,
/// winget Id, softwareupdate label, ...); `id` is `<source>:<key>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub source: USource,
    pub key: String,
    pub name: String,
    pub current: String,
    pub new: String,
    pub security: Option<bool>,
}

impl Item {
    pub fn new(source: USource, key: &str, name: &str, current: &str, new: &str) -> Item {
        Item {
            source,
            key: key.to_string(),
            name: name.to_string(),
            current: current.to_string(),
            new: new.to_string(),
            security: None,
        }
    }
    pub fn id(&self) -> String {
        format!("{}:{}", self.source.prefix(), self.key)
    }
}

// ---------------------------------------------------------------- apt

/// `apt list --upgradable`:
/// `firefox/noble-updates,noble-security 126.0 amd64 [upgradable from: 125.0]`
pub fn parse_apt(out: &str) -> Vec<Item> {
    let mut seen = HashSet::new();
    let mut v = Vec::new();
    for line in out.lines() {
        let line = line.trim();
        let mut t = line.split_whitespace();
        let Some(head) = t.next() else { continue };
        let Some((name, origins)) = head.split_once('/') else {
            continue;
        };
        let Some(new) = t.next() else { continue };
        if !valid_pkg_name(name) || !seen.insert(name.to_string()) {
            continue;
        }
        // Localised: "[upgradable from: X]" / "[aktualisierbar von: X]" - take the last word.
        let current = line
            .rfind('[')
            .and_then(|i| line[i + 1..].split(']').next())
            .and_then(|s| s.split_whitespace().last())
            .unwrap_or("")
            .to_string();
        let mut it = Item::new(USource::Apt, name, name, &current, new);
        it.security = Some(origins.to_lowercase().contains("-security"));
        v.push(it);
    }
    v
}

// ---------------------------------------------------------------- dnf

const RPM_ARCHES: &[&str] = &[
    "x86_64", "noarch", "i686", "i386", "aarch64", "armv7hl", "ppc64le", "s390x", "src",
];

fn split_arch(name_arch: &str) -> Option<(&str, &str)> {
    let (n, a) = name_arch.rsplit_once('.')?;
    RPM_ARCHES.contains(&a).then_some((n, a))
}

/// `dnf check-update` (exit status 100 means updates are available):
/// `name.arch  version-release  repo`, with an "Obsoleting Packages" section to ignore.
pub fn parse_dnf(out: &str) -> Vec<Item> {
    let mut seen = HashSet::new();
    let mut v = Vec::new();
    let mut pending: Option<&str> = None;
    for line in out.lines() {
        let t = line.trim();
        if t.starts_with("Obsoleting Packages") {
            break;
        }
        let toks: Vec<&str> = t.split_whitespace().collect();
        let (na, ver) = match (toks.len(), pending.take()) {
            (3, _) => (toks[0], toks[1]),
            // Old wrapped layout: long names on a line of their own.
            (2, Some(na)) => (na, toks[0]),
            (1, _) if split_arch(toks[0]).is_some() => {
                pending = Some(toks[0]);
                continue;
            }
            _ => continue,
        };
        let Some((name, _arch)) = split_arch(na) else {
            continue;
        };
        if !valid_pkg_name(na) || !seen.insert(na.to_string()) {
            continue;
        }
        v.push(Item::new(USource::Dnf, na, name, "", ver));
    }
    v
}

/// `rpm -q --queryformat %{NAME}.%{ARCH}\t%{VERSION}-%{RELEASE}\n ...` -> `name.arch` -> version.
pub fn parse_rpm_versions(out: &str) -> HashMap<String, String> {
    out.lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

// ---------------------------------------------------------------- pacman

/// `checkupdates` / `pacman -Qu`: `name old -> new` (lines marked `[ignored]` are skipped).
pub fn parse_pacman(out: &str) -> Vec<Item> {
    let mut seen = HashSet::new();
    let mut v = Vec::new();
    for line in out.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 4 || t[2] != "->" || t[4..].iter().any(|x| x.contains("ignored")) {
            continue;
        }
        if !valid_pkg_name(t[0]) || !seen.insert(t[0].to_string()) {
            continue;
        }
        v.push(Item::new(USource::Pacman, t[0], t[0], t[1], t[3]));
    }
    v
}

// ---------------------------------------------------------------- flatpak

/// `flatpak remote-ls --updates --app --columns=application,version`.
pub fn parse_flatpak_updates(out: &str) -> Vec<Item> {
    let mut seen = HashSet::new();
    let mut v = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let id = f[0].trim();
        if !id.contains('.') || !valid_pkg_name(id) || !seen.insert(id.to_string()) {
            continue;
        }
        let new = f.get(1).map(|s| s.trim()).unwrap_or("");
        v.push(Item::new(USource::Flatpak, id, id, "", new));
    }
    v
}

/// `flatpak list --app --columns=application,name,version` -> id -> (name, version).
pub fn parse_flatpak_installed(out: &str) -> HashMap<String, (String, String)> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let id = f[0].trim();
            id.contains('.').then(|| {
                (
                    id.to_string(),
                    (
                        f.get(1).map(|s| s.trim().to_string()).unwrap_or_default(),
                        f.get(2).map(|s| s.trim().to_string()).unwrap_or_default(),
                    ),
                )
            })
        })
        .collect()
}

// ---------------------------------------------------------------- snap

/// `snap refresh --list`: `Name Version Rev Size Publisher Notes`.
pub fn parse_snap_refresh(out: &str) -> Vec<Item> {
    let mut seen = HashSet::new();
    let mut v = Vec::new();
    for line in out.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 4 {
            continue;
        }
        // Header (any language) has a non-numeric revision column.
        if !t[2].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if !valid_pkg_name(t[0]) || !seen.insert(t[0].to_string()) {
            continue;
        }
        v.push(Item::new(USource::Snap, t[0], t[0], "", t[1]));
    }
    v
}

// ---------------------------------------------------------------- winget

fn char_width(c: char) -> usize {
    let u = c as u32;
    let wide = (0x1100..=0x115F).contains(&u)
        || (0x2E80..=0xA4CF).contains(&u)
        || (0xAC00..=0xD7A3).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFE30..=0xFE6F).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x1F300..=0x1FAFF).contains(&u);
    if wide {
        2
    } else if c.is_control() {
        0
    } else {
        1
    }
}

/// One `char` per display column (a wide character is followed by a `\0` filler).
fn grid(line: &str) -> Vec<char> {
    let mut g = Vec::with_capacity(line.len());
    for c in line.chars() {
        let w = char_width(c);
        if w == 0 {
            continue;
        }
        g.push(c);
        g.extend(std::iter::repeat_n('\0', w - 1));
    }
    g
}

fn cell(g: &[char], starts: &[usize], k: usize) -> String {
    let a = starts[k].min(g.len());
    let b = starts.get(k + 1).copied().unwrap_or(g.len()).min(g.len());
    g[a..b]
        .iter()
        .filter(|c| **c != '\0')
        .collect::<String>()
        .trim()
        .to_string()
}

/// Display-column start of every column of the header line. winget separates columns with a
/// single space when a header is the widest cell, so each header word starts a column; if a
/// language uses multi-word headers (more than five words) columns are split at 2+ spaces.
fn header_starts(header: &str) -> Vec<usize> {
    let g = grid(header);
    let words = |min_gap: usize| {
        let mut starts = Vec::new();
        let mut gap = min_gap;
        for (col, c) in g.iter().enumerate() {
            if *c == ' ' {
                gap += 1;
            } else if *c != '\0' {
                if gap >= min_gap && (col == 0 || g[col - 1] == ' ' || g[col - 1] == '\0') {
                    starts.push(col);
                }
                gap = 0;
            }
        }
        starts
    };
    let by_word = words(1);
    if by_word.len() <= 5 {
        by_word
    } else {
        words(2)
    }
}

fn is_separator(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 8 && t.chars().all(|c| c == '-')
}

/// `winget upgrade --include-unknown`. The table is fixed-width and localised, so columns
/// are located from the header line that precedes the `-----` separator; the first five
/// columns are Name, Id, Version, Available, Source in every language. Truncated cells
/// end in `…`. May contain several tables (the second one lists packages that need explicit
/// targeting).
pub fn parse_winget(out: &str) -> Vec<Item> {
    let lines = clean_terminal_lines(out);
    let mut v = Vec::new();
    let mut seen = HashSet::new();
    let mut i = 0;
    while i < lines.len() {
        if i == 0 || !is_separator(&lines[i]) {
            i += 1;
            continue;
        }
        let starts = header_starts(&lines[i - 1]);
        if starts.len() < 4 {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < lines.len() && !lines[j].trim().is_empty() && !is_separator(&lines[j]) {
            let g = grid(&lines[j]);
            // Rows have whitespace right before every column start; footer sentences
            // ("3 upgrades available.") generally do not, and are too short to fill the
            // Id / Available cells.
            let aligned = starts[1..].iter().all(|p| match g.get(p - 1) {
                None => true,
                Some(c) => c.is_whitespace() || *c == '\0',
            });
            if aligned {
                let (name, id) = (cell(&g, &starts, 0), cell(&g, &starts, 1));
                let (ver, avail) = (cell(&g, &starts, 2), cell(&g, &starts, 3));
                if !id.is_empty()
                    && !avail.is_empty()
                    && !id.contains(' ')
                    && seen.insert(id.clone())
                {
                    v.push(Item::new(USource::Winget, &id, &name, &ver, &avail));
                }
            }
            j += 1;
        }
        i = j;
    }
    v
}

// ---------------------------------------------------------------- brew

fn versions_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str())
            .next_back()
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

/// `brew outdated --json=v2`. Pinned formulae are skipped (`brew upgrade` skips them).
pub fn parse_brew_outdated(out: &str) -> Result<Vec<Item>, String> {
    let t = out.trim();
    if t.is_empty() {
        return Ok(Vec::new());
    }
    let v: Value =
        serde_json::from_str(t).map_err(|e| format!("unexpected `brew outdated` output: {e}"))?;
    let mut items = Vec::new();
    for (key, source) in [("formulae", USource::Brew), ("casks", USource::BrewCask)] {
        let Some(arr) = v.get(key).and_then(Value::as_array) else {
            continue;
        };
        for e in arr {
            let Some(name) = e.get("name").and_then(Value::as_str) else {
                continue;
            };
            if !valid_pkg_name(name) || e.get("pinned").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let cur = e
                .get("installed_versions")
                .map(versions_of)
                .unwrap_or_default();
            let new = e
                .get("current_version")
                .and_then(Value::as_str)
                .unwrap_or("");
            items.push(Item::new(source, name, name, &cur, new));
        }
    }
    Ok(items)
}

// ---------------------------------------------------------------- softwareupdate

/// An entry of `softwareupdate -l` before it is turned into a list item.
#[derive(Debug, Clone, PartialEq)]
pub struct MacUpdate {
    pub label: String,
    pub title: String,
    pub version: String,
    pub restart: bool,
}

fn kv_fields(s: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for part in s.split(", ") {
        if let Some((k, v)) = part.split_once(':') {
            m.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    m
}

/// `softwareupdate -l` in both the current (`* Label: ...` + `Title: ..., Version: ...`) and
/// the older (`* label` + `Title (version), 1234K [recommended] [restart]`) layouts.
pub fn parse_softwareupdate(out: &str) -> Vec<MacUpdate> {
    let mut v = Vec::new();
    let lines: Vec<&str> = out.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        let Some(rest) = t.strip_prefix("* ") else {
            i += 1;
            continue;
        };
        let (label, new_format) = match rest.strip_prefix("Label:") {
            Some(l) => (l.trim().to_string(), true),
            None => (rest.trim().to_string(), false),
        };
        let detail = lines.get(i + 1).map(|l| l.trim()).unwrap_or("");
        i += 1;
        if label.is_empty() || label.starts_with('-') || label.chars().any(|c| c.is_control()) {
            continue;
        }
        let mut u = MacUpdate {
            label,
            title: String::new(),
            version: String::new(),
            restart: false,
        };
        if new_format || detail.starts_with("Title:") {
            let f = kv_fields(detail);
            u.title = f.get("Title").cloned().unwrap_or_default();
            u.version = f.get("Version").cloned().unwrap_or_default();
            u.restart = f.get("Action").is_some_and(|a| a.contains("restart"));
        } else {
            // "iTunes (12.13.0), 258790K [recommended] [restart]"
            u.restart = detail.contains("[restart]");
            let head = detail.split(", ").next().unwrap_or("");
            if let (Some(a), Some(b)) = (head.rfind('('), head.rfind(')')) {
                if a < b {
                    u.title = head[..a].trim().to_string();
                    u.version = head[a + 1..b].trim().to_string();
                }
            }
        }
        if u.title.is_empty() {
            u.title = u.label.clone();
        }
        v.push(u);
    }
    v
}

pub fn macos_items(updates: &[MacUpdate]) -> Vec<Item> {
    updates
        .iter()
        .map(|u| Item::new(USource::Macos, &u.label, &u.title, "", &u.version))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `apt list --upgradable` on Ubuntu 24.04 (stdout).
    const APT: &str = "\
Listing...
firefox/noble-updates,noble-security 126.0+build2-0ubuntu0.24.04.1 amd64 [upgradable from: 125.0.3+build1-0ubuntu0.24.04.1]
libssl3t64/noble-updates,noble-security 3.0.13-0ubuntu3.1 amd64 [upgradable from: 3.0.13-0ubuntu3]
vim/noble-updates 2:9.1.0016-1ubuntu7.2 amd64 [upgradable from: 2:9.1.0016-1ubuntu7]
libfoo/noble-updates 1.2 i386 [upgradable from: 1.1]
libfoo/noble-updates 1.2 amd64 [upgradable from: 1.1]
";

    #[test]
    fn apt_parses_versions_and_security_origin() {
        let v = parse_apt(APT);
        assert_eq!(v.len(), 4);
        assert_eq!(v[0].key, "firefox");
        assert_eq!(v[0].current, "125.0.3+build1-0ubuntu0.24.04.1");
        assert_eq!(v[0].new, "126.0+build2-0ubuntu0.24.04.1");
        assert_eq!(v[0].security, Some(true));
        assert_eq!(v[0].id(), "apt:firefox");
        assert_eq!(v[2].security, Some(false));
        assert_eq!(v[2].new, "2:9.1.0016-1ubuntu7.2");
        assert_eq!(v[3].key, "libfoo"); // multi-arch duplicate collapsed
    }

    #[test]
    fn apt_edge_cases() {
        assert!(parse_apt("").is_empty());
        assert!(parse_apt("Listing... Done\n").is_empty());
        // German locale
        let de = "Auflistung...\nfoo/jammy-security 2.0 amd64 [aktualisierbar von: 1.0]\n";
        let v = parse_apt(de);
        assert_eq!(v[0].current, "1.0");
        assert_eq!(v[0].security, Some(true));
        // Debian origin naming
        let v = parse_apt("bar/stable-security 1 all [upgradable from: 0]\n");
        assert_eq!(v[0].security, Some(true));
        // no bracket
        let v = parse_apt("baz/noble 1.0 amd64\n");
        assert_eq!(v[0].current, "");
        // hostile names are dropped
        assert!(parse_apt("-y/noble 1.0 amd64 [upgradable from: 0]\n").is_empty());
    }

    /// `dnf check-update` on Fedora 40 (exit code 100).
    const DNF: &str = "\
Last metadata expiration check: 0:12:34 ago on Mon 20 May 2024 10:00:00 AM UTC.

firefox.x86_64                          126.0-1.fc40                     updates
kernel-core.x86_64                      6.8.9-300.fc40                   updates
NetworkManager-libnm.x86_64             1:1.46.0-2.fc40                  updates-testing
glibc-langpack-en.x86_64                2.39-17.fc40                     updates
python3-foo.noarch                      1.2-3.fc40                       fedora

Obsoleting Packages
newthing.x86_64                         2.0-1.fc40                       updates
    oldthing.x86_64                     1.0-1.fc40                       @fedora
";

    #[test]
    fn dnf_parses_table_and_stops_at_obsoleting() {
        let v = parse_dnf(DNF);
        let keys: Vec<&str> = v.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "firefox.x86_64",
                "kernel-core.x86_64",
                "NetworkManager-libnm.x86_64",
                "glibc-langpack-en.x86_64",
                "python3-foo.noarch"
            ]
        );
        assert_eq!(v[0].name, "firefox");
        assert_eq!(v[0].new, "126.0-1.fc40");
        assert_eq!(v[2].new, "1:1.46.0-2.fc40");
        assert_eq!(v[0].id(), "dnf:firefox.x86_64");
    }

    #[test]
    fn dnf_edge_cases() {
        assert!(parse_dnf("").is_empty());
        assert!(parse_dnf("Last metadata expiration check: 0:00:01 ago on Mon.\n").is_empty());
        // yum-style wrapped row
        let wrapped = "very-long-package-name-that-wraps.x86_64\n     1.0-1.fc40   updates\n";
        let v = parse_dnf(wrapped);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].new, "1.0-1.fc40");
        assert_eq!(v[0].name, "very-long-package-name-that-wraps");
    }

    #[test]
    fn rpm_versions_map() {
        let m = parse_rpm_versions("firefox.x86_64\t125.0-1.fc40\npackage nothing is not installed\nkernel-core.x86_64\t6.8.5-301.fc40\n");
        assert_eq!(m["firefox.x86_64"], "125.0-1.fc40");
        assert_eq!(m.len(), 2);
    }

    /// `checkupdates` on Arch.
    const PACMAN: &str = "\
linux 6.8.9.arch1-1 -> 6.8.10.arch1-1
firefox 125.0.3-1 -> 126.0-1
ignoredpkg 1.0-1 -> 2.0-1 [ignored]
";

    #[test]
    fn pacman_parses_arrows_and_skips_ignored() {
        let v = parse_pacman(PACMAN);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].key, "linux");
        assert_eq!(v[0].current, "6.8.9.arch1-1");
        assert_eq!(v[0].new, "6.8.10.arch1-1");
        assert!(parse_pacman("").is_empty());
        assert!(parse_pacman("garbage\n").is_empty());
    }

    #[test]
    fn flatpak_updates_and_installed() {
        let v = parse_flatpak_updates(
            "org.mozilla.firefox\t126.0\norg.gimp.GIMP\t\nApplication ID\tVersion\n",
        );
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].new, "126.0");
        assert_eq!(v[1].new, "");
        let m = parse_flatpak_installed("org.mozilla.firefox\tFirefox\t125.0\n");
        assert_eq!(
            m["org.mozilla.firefox"],
            ("Firefox".to_string(), "125.0".to_string())
        );
        assert!(parse_flatpak_updates("").is_empty());
    }

    /// `snap refresh --list`.
    const SNAP: &str = "\
Name     Version  Rev   Size   Publisher   Notes
firefox  127.0    4400  250MB  mozilla**   -
core22   20240416 1380  74MB   canonical** base
";

    #[test]
    fn snap_refresh_list() {
        let v = parse_snap_refresh(SNAP);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].key, "firefox");
        assert_eq!(v[0].new, "127.0");
        assert!(parse_snap_refresh("").is_empty());
        assert!(parse_snap_refresh("All snaps up to date.\n").is_empty());
    }

    /// `winget upgrade --include-unknown` (English, 120 columns, with progress spinner).
    fn winget_sample() -> String {
        let mut s = String::from("\u{feff}   - \r   \\ \r   | \r   / \r");
        s.push_str("Name                        Id                       Version        Available      Source\r\n");
        s.push_str(&"-".repeat(84));
        s.push_str("\r\n");
        s.push_str("Mozilla Firefox (x64 en-US) Mozilla.Firefox          125.0.3        126.0          winget\r\n");
        s.push_str("Git                         Git.Git                  2.44.0         2.45.1         winget\r\n");
        s.push_str("Microsoft Visual Studio Co… Microsoft.VisualStudio.… 17.9.1         17.10.0        winget\r\n");
        s.push_str("Some Local App              Vendor.LocalApp          Unknown        1.2.3\r\n");
        s.push_str("3 upgrades available.\r\n");
        s.push_str("\r\n");
        s.push_str("The following packages have an upgrade available, but require explicit targeting for upgrade:\r\n");
        s.push_str("Name        Id              Version   Available Source\r\n");
        s.push_str(&"-".repeat(56));
        s.push_str("\r\n");
        s.push_str("Pinned App  Vendor.Pinned   1.0       1.1       winget\r\n");
        s.push_str("1 package(s) have pins that prevent upgrade.\r\n");
        s
    }

    #[test]
    fn winget_parses_columns_truncation_spinner_and_second_table() {
        let v = parse_winget(&winget_sample());
        let ids: Vec<&str> = v.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(
            ids,
            [
                "Mozilla.Firefox",
                "Git.Git",
                "Microsoft.VisualStudio.…",
                "Vendor.LocalApp",
                "Vendor.Pinned"
            ]
        );
        assert_eq!(v[0].name, "Mozilla Firefox (x64 en-US)");
        assert_eq!(v[0].current, "125.0.3");
        assert_eq!(v[0].new, "126.0");
        assert_eq!(v[0].id(), "winget:Mozilla.Firefox");
        assert_eq!(v[2].name, "Microsoft Visual Studio Co…");
        assert_eq!(v[3].current, "Unknown");
        assert_eq!(v[3].new, "1.2.3");
        assert_eq!(v[4].new, "1.1");
    }

    #[test]
    fn winget_localised_header_and_no_updates() {
        // Spanish: header words differ but the layout is the same.
        let es = "Nombre        Id            Versión  Disponible  Origen\n\
                  ---------------------------------------------------\n\
                  Foo App       Vendor.Foo    1.0      1.1         winget\n\
                  1 actualizaciones disponibles.\n";
        let v = parse_winget(es);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].key, "Vendor.Foo");
        assert_eq!(v[0].new, "1.1");
        assert!(parse_winget("").is_empty());
        assert!(parse_winget("No installed package found matching input criteria.\n").is_empty());
        assert!(parse_winget("   - \r   \\ \r").is_empty());
    }

    #[test]
    fn winget_wide_characters_keep_alignment() {
        // CJK characters take two columns.
        let s = "Name              Id            Version  Available  Source\n\
                 ------------------------------------------------------\n\
                 日本語アプリ      Vendor.Jp     1.0      2.0        winget\n";
        let v = parse_winget(s);
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].key, "Vendor.Jp");
        assert_eq!(v[0].name, "日本語アプリ");
        assert_eq!(v[0].current, "1.0");
    }

    const BREW_JSON: &str = r#"{"formulae":[{"name":"wget","installed_versions":["1.24.5"],"current_version":"1.25.0","pinned":false,"pinned_version":null},{"name":"pinned-thing","installed_versions":["1.0"],"current_version":"2.0","pinned":true,"pinned_version":"1.0"}],"casks":[{"name":"firefox","installed_versions":"125.0.1","current_version":"126.0"},{"name":"iterm2","installed_versions":["3.5.1"],"current_version":"3.5.2"}]}"#;

    #[test]
    fn brew_outdated_json() {
        let v = parse_brew_outdated(BREW_JSON).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].id(), "brew:wget");
        assert_eq!(v[0].current, "1.24.5");
        assert_eq!(v[0].new, "1.25.0");
        assert_eq!(v[1].id(), "brew-cask:firefox");
        assert_eq!(v[1].current, "125.0.1");
        assert_eq!(v[2].current, "3.5.1");
        assert!(parse_brew_outdated("").unwrap().is_empty());
        assert!(parse_brew_outdated("{}").unwrap().is_empty());
        assert!(parse_brew_outdated("{\"formulae\":[],\"casks\":[]}")
            .unwrap()
            .is_empty());
        assert!(parse_brew_outdated("not json").is_err());
    }

    const SU_NEW: &str = "\
Software Update Tool

Finding available software
Software Update found the following new or updated software:
* Label: Safari17.5VenturaAuto-17.5
\tTitle: Safari, Version: 17.5, Size: 123456KiB, Recommended: YES,
* Label: macOS Sonoma 14.5-23F79
\tTitle: macOS Sonoma 14.5, Version: 14.5, Size: 1234567KiB, Recommended: YES, Action: restart,
";

    const SU_OLD: &str = "\
Software Update Tool
Copyright 2002-2010 Apple

Finding available software

Software Update found the following new or updated software:
   * iTunesX-12.13.0
\tiTunes (12.13.0), 258790K [recommended]
   * macOS Ventura 13.6.6-22G630
\tmacOS Ventura 13.6.6 (13.6.6), 1234567K [recommended] [restart]
";

    #[test]
    fn softwareupdate_both_formats() {
        let v = parse_softwareupdate(SU_NEW);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].label, "Safari17.5VenturaAuto-17.5");
        assert_eq!(v[0].title, "Safari");
        assert_eq!(v[0].version, "17.5");
        assert!(!v[0].restart);
        assert_eq!(v[1].label, "macOS Sonoma 14.5-23F79");
        assert!(v[1].restart);
        let o = parse_softwareupdate(SU_OLD);
        assert_eq!(o.len(), 2);
        assert_eq!(o[0].label, "iTunesX-12.13.0");
        assert_eq!(o[0].title, "iTunes");
        assert_eq!(o[0].version, "12.13.0");
        assert!(o[1].restart);
        assert_eq!(o[1].label, "macOS Ventura 13.6.6-22G630");
        let items = macos_items(&v);
        assert_eq!(items[1].id(), "macos:macOS Sonoma 14.5-23F79");
    }

    #[test]
    fn softwareupdate_none_and_hostile_labels() {
        assert!(parse_softwareupdate(
            "Software Update Tool\n\nFinding available software\nNo new software available.\n"
        )
        .is_empty());
        assert!(parse_softwareupdate("").is_empty());
        assert!(parse_softwareupdate("* Label: --restart\n\tTitle: x, Version: 1\n").is_empty());
    }
}
