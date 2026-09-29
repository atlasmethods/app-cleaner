//! Installed-application discovery for Linux package managers (and Homebrew / AppImages,
//! which the macOS collector shares). Everything here parses tool output, so it is
//! unit-tested on any host.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::ctx::Ctx;
use crate::features::uninstall::model::{Action, Found, Source};
use crate::pkgutil::{date_from_unix, parse_human_size, strip_email, valid_pkg_name};

/// The exact `dpkg-query` invocation (dpkg expands the `\t` / `\n` escapes itself).
pub const DPKG_FORMAT: &str = "-f=${Package}\\t${Version}\\t${Installed-Size}\\t${Maintainer}\\t${Priority}\\t${Essential}\\t${db:Status-Status}\\n";
pub const RPM_FORMAT: &str =
    "%{NAME}\\t%{VERSION}-%{RELEASE}\\t%{SIZE}\\t%{VENDOR}\\t%{INSTALLTIME}\\n";
pub const FLATPAK_COLUMNS: &str = "--columns=application,name,version,size,origin";

// ---------------------------------------------------------------- dpkg

/// `dpkg-query -W -f=...` output. Only fully `installed` packages; `Essential: yes` or
/// `Priority: required` marks a system package.
pub fn parse_dpkg(out: &str, ctx: &Ctx) -> Vec<Found> {
    let mut seen = HashSet::new();
    let mut v = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 7 || f[6].trim() != "installed" {
            continue;
        }
        let name = f[0].trim();
        if !valid_pkg_name(name) || !seen.insert(name.to_string()) {
            continue;
        }
        let mut e = Found::new(
            format!("dpkg:{name}"),
            name.to_string(),
            f[1].trim().to_string(),
            Source::Dpkg,
            Action::Dpkg(name.to_string()),
        );
        e.entry.publisher = strip_email(f[3]);
        e.entry.size_bytes = f[2].trim().parse::<u64>().ok().map(|kib| kib * 1024);
        e.entry.is_system = f[5].trim().eq_ignore_ascii_case("yes") || f[4].trim() == "required";
        e.entry.install_date = dpkg_install_date(ctx, name);
        v.push(e);
    }
    v
}

/// dpkg records no install time; the mtime of the package's file list is when it was unpacked.
fn dpkg_install_date(ctx: &Ctx, name: &str) -> Option<String> {
    let dir = ctx.env.sys_path("/var/lib/dpkg/info");
    for cand in [format!("{name}.list"), format!("{name}:amd64.list")] {
        if let Ok(m) = fs::metadata(dir.join(&cand)) {
            if let Ok(t) = m.modified() {
                let secs = t
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                return date_from_unix(secs);
            }
        }
    }
    None
}

// ---------------------------------------------------------------- rpm

const RPM_CRITICAL: &[&str] = &[
    "glibc",
    "glibc-common",
    "glibc-minimal-langpack",
    "bash",
    "systemd",
    "systemd-libs",
    "systemd-udev",
    "rpm",
    "rpm-libs",
    "dnf",
    "dnf5",
    "yum",
    "libdnf",
    "librepo",
    "libsolv",
    "coreutils",
    "filesystem",
    "setup",
    "basesystem",
    "shadow-utils",
    "util-linux",
    "sudo",
    "polkit",
    "dbus",
    "dbus-broker",
    "python3",
    "python3-libs",
    "openssl-libs",
    "ca-certificates",
    "crypto-policies",
    "selinux-policy",
    "selinux-policy-targeted",
    "grub2-common",
    "grub2-pc",
    "grub2-efi-x64",
    "NetworkManager",
    "fedora-release",
    "fedora-release-common",
    "redhat-release",
    "centos-release",
    "rocky-release",
    "almalinux-release",
];

fn rpm_is_critical(name: &str) -> bool {
    RPM_CRITICAL.contains(&name)
        || name == "kernel"
        || name.starts_with("kernel-core")
        || name.starts_with("kernel-modules")
        || name == "linux-firmware"
}

/// `rpm -qa --queryformat %{NAME}\t%{VERSION}-%{RELEASE}\t%{SIZE}\t%{VENDOR}\t%{INSTALLTIME}\n`.
pub fn parse_rpm(out: &str) -> Vec<Found> {
    let mut seen = HashSet::new();
    let mut v = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 3 {
            continue;
        }
        let name = f[0].trim();
        // gpg-pubkey entries are imported keys, not software.
        if name == "gpg-pubkey" || !valid_pkg_name(name) || !seen.insert(name.to_string()) {
            continue;
        }
        let mut e = Found::new(
            format!("rpm:{name}"),
            name.to_string(),
            f[1].trim().to_string(),
            Source::Rpm,
            Action::Rpm(name.to_string()),
        );
        e.entry.size_bytes = f[2].trim().parse().ok();
        let vendor = f.get(3).map(|s| s.trim()).unwrap_or("");
        e.entry.publisher = if vendor == "(none)" {
            String::new()
        } else {
            vendor.to_string()
        };
        e.entry.install_date = f
            .get(4)
            .and_then(|s| s.trim().parse::<i64>().ok())
            .and_then(date_from_unix);
        e.entry.is_system = rpm_is_critical(name);
        v.push(e);
    }
    v
}

// ---------------------------------------------------------------- pacman

const PACMAN_CRITICAL: &[&str] = &[
    "base",
    "linux",
    "linux-lts",
    "linux-zen",
    "linux-hardened",
    "linux-firmware",
    "glibc",
    "pacman",
    "systemd",
    "systemd-libs",
    "bash",
    "coreutils",
    "filesystem",
    "util-linux",
    "util-linux-libs",
    "gcc-libs",
    "openssl",
    "sudo",
    "shadow",
    "grub",
    "efibootmgr",
    "archlinux-keyring",
    "ca-certificates",
    "ca-certificates-mozilla",
    "ca-certificates-utils",
    "iana-etc",
    "licenses",
    "pacman-mirrorlist",
    "tzdata",
    "zlib",
    "readline",
    "ncurses",
];

/// `pacman -Qi` (C locale): blocks of `Key : Value` separated by blank lines.
pub fn parse_pacman(out: &str) -> Vec<Found> {
    let mut v = Vec::new();
    let mut seen = HashSet::new();
    for block in out.split("\n\n") {
        let mut name = String::new();
        let mut version = String::new();
        let mut size = None;
        let mut publisher = String::new();
        let mut date = None;
        let mut groups = String::new();
        for line in block.lines() {
            // Continuation lines start with whitespace; keys are padded before the colon.
            if line.starts_with(char::is_whitespace) {
                continue;
            }
            let Some((k, val)) = line.split_once(" : ") else {
                continue;
            };
            let val = val.trim();
            match k.trim() {
                "Name" => name = val.to_string(),
                "Version" => version = val.to_string(),
                "Installed Size" => size = parse_human_size(val),
                "Packager" => publisher = strip_email(val),
                "Install Date" => date = parse_pacman_date(val),
                "Groups" => groups = val.to_string(),
                _ => {}
            }
        }
        if name.is_empty() || !valid_pkg_name(&name) || !seen.insert(name.clone()) {
            continue;
        }
        let mut e = Found::new(
            format!("pacman:{name}"),
            name.clone(),
            version,
            Source::Pacman,
            Action::Pacman(name.clone()),
        );
        e.entry.publisher = if publisher == "Unknown Packager" {
            String::new()
        } else {
            publisher
        };
        e.entry.size_bytes = size;
        e.entry.install_date = date;
        e.entry.is_system = PACMAN_CRITICAL.contains(&name.as_str())
            || groups
                .split_whitespace()
                .any(|g| g == "base" || g == "base-devel");
        v.push(e);
    }
    v
}

/// `Mon 01 Jan 2024 10:00:00 AM UTC` -> `2024-01-01`.
fn parse_pacman_date(s: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let t: Vec<&str> = s.split_whitespace().collect();
    // [weekday, day, month, year, ...]
    if t.len() < 4 {
        return None;
    }
    let day: u32 = t[1].parse().ok()?;
    let month = MONTHS.iter().position(|m| *m == t[2])? as u32 + 1;
    let year: u32 = t[3].parse().ok()?;
    (1..=31)
        .contains(&day)
        .then(|| format!("{year:04}-{month:02}-{day:02}"))
}

// ---------------------------------------------------------------- flatpak

/// `flatpak list --app --columns=application,name,version,size,origin` (tab separated).
pub fn parse_flatpak(out: &str) -> Vec<Found> {
    let mut seen = HashSet::new();
    let mut v = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let id = f[0].trim();
        // Skips a header row ("Application ID") and anything that is not an app id.
        if !id.contains('.') || !valid_pkg_name(id) || !seen.insert(id.to_string()) {
            continue;
        }
        let name = f
            .get(1)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or(id);
        let mut e = Found::new(
            format!("flatpak:{id}"),
            name.to_string(),
            f.get(2).map(|s| s.trim().to_string()).unwrap_or_default(),
            Source::Flatpak,
            Action::Flatpak(id.to_string()),
        );
        e.entry.size_bytes = f.get(3).and_then(|s| parse_human_size(s));
        e.entry.publisher = f.get(4).map(|s| s.trim().to_string()).unwrap_or_default();
        v.push(e);
    }
    v
}

// ---------------------------------------------------------------- snap

const SNAP_BASES: &[&str] = &["snapd", "core", "bare"];

/// `snap list`: `Name Version Rev Tracking Publisher Notes`.
pub fn parse_snap(out: &str) -> Vec<Found> {
    let mut v = Vec::new();
    let mut seen = HashSet::new();
    for (i, line) in out.lines().enumerate() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 5 {
            continue;
        }
        if i == 0 && t[0].eq_ignore_ascii_case("name") {
            continue; // header (any other locale's header is caught by the check below)
        }
        let name = t[0];
        // Column 3 is the numeric revision (or `x1` for local snaps); a header row has none.
        let rev_ok = t[2]
            .trim_start_matches('x')
            .chars()
            .all(|c| c.is_ascii_digit());
        if !rev_ok || !valid_pkg_name(name) || !seen.insert(name.to_string()) {
            continue;
        }
        let notes = t.get(5).copied().unwrap_or("-");
        let mut e = Found::new(
            format!("snap:{name}"),
            name.to_string(),
            t[1].to_string(),
            Source::Snap,
            Action::Snap(name.to_string()),
        );
        e.entry.publisher = t[4].trim_end_matches('*').trim_end_matches('✓').to_string();
        if e.entry.publisher == "-" {
            e.entry.publisher.clear();
        }
        e.entry.is_system = SNAP_BASES.contains(&name)
            || (name.starts_with("core") && name[4..].chars().all(|c| c.is_ascii_digit()))
            || notes.split(',').any(|n| n == "base" || n == "snapd");
        v.push(e);
    }
    v
}

// ---------------------------------------------------------------- AppImage

/// Split `Foo-Bar-1.2.3-x86_64` into (`Foo Bar`, `1.2.3`).
pub fn split_appimage_name(stem: &str) -> (String, String) {
    let mut s = stem.to_string();
    for arch in [
        "-x86_64", "_x86_64", ".x86_64", "-x86-64", "-amd64", "_amd64", "-aarch64", "-arm64",
    ] {
        if let Some(p) = s.to_lowercase().rfind(arch) {
            if p + arch.len() == s.len() {
                s.truncate(p);
            }
        }
    }
    let tokens: Vec<&str> = s.split(['-', '_']).filter(|t| !t.is_empty()).collect();
    let is_version = |t: &str| {
        let t = t.strip_prefix(['v', 'V']).unwrap_or(t);
        t.starts_with(|c: char| c.is_ascii_digit())
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '+')
            && (t.contains('.') || t.chars().all(|c| c.is_ascii_digit()))
    };
    match tokens.iter().position(|t| is_version(t)) {
        Some(0) | None => (tokens.join(" "), String::new()),
        Some(i) => (
            tokens[..i].join(" "),
            tokens[i..]
                .join("-")
                .trim_start_matches(['v', 'V'])
                .to_string(),
        ),
    }
}

/// `*.AppImage` files in `~/Applications` and `~/.local/bin`.
pub fn scan_appimages(ctx: &Ctx) -> Vec<Found> {
    let mut v = Vec::new();
    for dir in [
        ctx.env.home.join("Applications"),
        ctx.env.home.join(".local").join("bin"),
    ] {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        files.sort_by_key(|e| e.file_name());
        for e in files {
            let fname = e.file_name().to_string_lossy().into_owned();
            if !fname.to_lowercase().ends_with(".appimage") {
                continue;
            }
            // Symlinks are skipped: removing one would not remove the application.
            let Ok(meta) = fs::symlink_metadata(e.path()) else {
                continue;
            };
            if !meta.file_type().is_file() {
                continue;
            }
            let stem = &fname[..fname.len() - ".appimage".len()];
            let (name, version) = split_appimage_name(stem);
            let name = if name.is_empty() {
                stem.to_string()
            } else {
                name
            };
            let path = e.path();
            let mut f = Found::new(
                format!("appimage:{}", path.display()),
                name,
                version,
                Source::Appimage,
                Action::AppImage(path.clone()),
            );
            f.entry.size_bytes = Some(meta.len());
            f.entry.install_date = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| date_from_unix(d.as_secs() as i64));
            v.push(f);
        }
    }
    v
}

/// The package (`Path`) of an AppImage id.
pub fn appimage_path_of(id: &str) -> Option<&Path> {
    id.strip_prefix("appimage:").map(Path::new)
}

// ---------------------------------------------------------------- Homebrew

/// `brew list --versions` / `brew list --cask --versions`: `name v1 [v2 ...]`.
pub fn parse_brew(out: &str, cask: bool) -> Vec<Found> {
    let mut v = Vec::new();
    let mut seen = HashSet::new();
    for line in out.lines() {
        let mut t = line.split_whitespace();
        let Some(name) = t.next() else { continue };
        // Multiple installed versions: the last one is the newest.
        let version = t.last().unwrap_or("").to_string();
        if !valid_pkg_name(name) || !seen.insert(name.to_string()) {
            continue;
        }
        let kind = if cask { "brew-cask" } else { "brew" };
        v.push(Found::new(
            format!("{kind}:{name}"),
            name.to_string(),
            version,
            Source::Brew,
            Action::Brew {
                name: name.to_string(),
                cask,
            },
        ));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::MockRunner;

    fn ctx() -> (tempfile::TempDir, Ctx) {
        let d = tempfile::tempdir().unwrap();
        let c = Ctx::test(d.path(), MockRunner::new());
        fs::create_dir_all(&c.env.home).unwrap();
        (d, c)
    }

    /// Captured from `dpkg-query -W -f=...` on Ubuntu 24.04 (trimmed).
    const DPKG: &str = "\
adduser\t3.137ubuntu1\t608\tUbuntu Core Developers <ubuntu-core-dev@lists.ubuntu.com>\timportant\t\tinstalled
apt\t2.7.14build2\t4084\tUbuntu Developers <ubuntu-devel-discuss@lists.ubuntu.com>\trequired\t\tinstalled
base-files\t13ubuntu10\t394\tUbuntu Developers <ubuntu-devel-discuss@lists.ubuntu.com>\trequired\tyes\tinstalled
bash\t5.2.21-2ubuntu4\t1844\tUbuntu Developers <ubuntu-devel-discuss@lists.ubuntu.com>\trequired\tyes\tinstalled
firefox\t126.0+build2-0ubuntu0.24.04.1\t257984\tUbuntu Mozilla Team <ubuntu-mozilla-devel@lists.ubuntu.com>\toptional\t\tinstalled
gimp\t2.10.36-3build2\t28320\tUbuntu Developers <ubuntu-devel-discuss@lists.ubuntu.com>\toptional\t\tinstalled
libfoo1:amd64\t1.0\t\t\textra\t\tinstalled
oldthing\t1.0-1\t100\tSomeone <a@b.c>\toptional\t\tconfig-files
half\t1.0-1\t100\tSomeone <a@b.c>\toptional\t\thalf-installed
nomaint\t0.1\t12\t\toptional\t\tinstalled
";

    #[test]
    fn dpkg_parses_installed_only_with_system_flags() {
        let (_d, c) = ctx();
        let v = parse_dpkg(DPKG, &c);
        let names: Vec<&str> = v.iter().map(|f| f.entry.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "adduser",
                "apt",
                "base-files",
                "bash",
                "firefox",
                "gimp",
                "libfoo1:amd64",
                "nomaint"
            ]
        );
        let ff = &v[4].entry;
        assert_eq!(ff.id, "dpkg:firefox");
        assert_eq!(ff.version, "126.0+build2-0ubuntu0.24.04.1");
        assert_eq!(ff.publisher, "Ubuntu Mozilla Team");
        assert_eq!(ff.size_bytes, Some(257984 * 1024));
        assert!(!ff.is_system && ff.uninstallable && !ff.can_repair && !ff.can_modify);
        assert_eq!(v[4].action, Action::Dpkg("firefox".into()));
        // required priority and Essential: yes are system; important is not
        assert!(!v[0].entry.is_system);
        assert!(v[1].entry.is_system);
        assert!(v[2].entry.is_system);
        assert!(v[3].entry.is_system);
        // missing size / maintainer
        assert_eq!(v[6].entry.size_bytes, None);
        assert_eq!(v[7].entry.publisher, "");
    }

    #[test]
    fn dpkg_edge_cases() {
        let (_d, c) = ctx();
        assert!(parse_dpkg("", &c).is_empty());
        assert!(parse_dpkg("\n\n", &c).is_empty());
        assert!(parse_dpkg("garbage without tabs", &c).is_empty());
        // option-like names never become actionable
        assert!(parse_dpkg("-y\t1\t1\tm\toptional\t\tinstalled\n", &c).is_empty());
        // duplicate names (multi-arch) collapse
        let two =
            "libc6\t2.39\t1\tm\trequired\t\tinstalled\nlibc6\t2.39\t1\tm\trequired\t\tinstalled\n";
        assert_eq!(parse_dpkg(two, &c).len(), 1);
    }

    #[test]
    fn dpkg_install_date_from_info_list() {
        let (_d, c) = ctx();
        let dir = c.env.sys_path("/var/lib/dpkg/info");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("firefox.list"), "x").unwrap();
        let v = parse_dpkg(DPKG, &c);
        let d = v.iter().find(|f| f.entry.name == "firefox").unwrap();
        assert_eq!(d.entry.install_date.as_deref().map(str::len), Some(10));
        let none = v.iter().find(|f| f.entry.name == "gimp").unwrap();
        assert_eq!(none.entry.install_date, None);
    }

    /// `rpm -qa --queryformat ...` on Fedora 40.
    const RPM: &str = "\
glibc\t2.39-17.fc40\t6740594\tFedora Project\t1714000000
firefox\t126.0-1.fc40\t265091321\tFedora Project\t1716000000
kernel-core\t6.8.5-301.fc40\t73456778\tFedora Project\t1714000100
gpg-pubkey\t105ef944\t0\t(none)\t1700000000
gpg-pubkey\t3b6ad3d4\t0\t(none)\t1700000001
localthing\t1.0-1\t1234\t(none)\t(none)
bad line
";

    #[test]
    fn rpm_parses_and_skips_gpg_pubkeys() {
        let v = parse_rpm(RPM);
        let names: Vec<&str> = v.iter().map(|f| f.entry.name.as_str()).collect();
        assert_eq!(names, ["glibc", "firefox", "kernel-core", "localthing"]);
        assert!(v[0].entry.is_system);
        assert!(!v[1].entry.is_system);
        assert!(v[2].entry.is_system);
        assert_eq!(v[1].entry.id, "rpm:firefox");
        assert_eq!(v[1].entry.publisher, "Fedora Project");
        assert_eq!(v[1].entry.size_bytes, Some(265_091_321));
        assert_eq!(v[1].entry.install_date.as_deref(), Some("2024-05-18"));
        assert_eq!(v[3].entry.publisher, "");
        assert_eq!(v[3].entry.install_date, None);
        assert!(parse_rpm("").is_empty());
    }

    /// `env LC_ALL=C pacman -Qi` on Arch (two packages, trimmed).
    const PACMAN: &str = "\
Name            : firefox
Version         : 126.0-1
Description     : Fast, Private & Safe Web Browser
Architecture    : x86_64
URL             : https://www.mozilla.org/firefox/
Licenses        : MPL-2.0
Groups          : None
Provides        : None
Depends On      : gtk3  libxt  mime-types
Optional Deps   : ffmpeg: H.264/AAC/MP3 decoding
                  networkmanager: Location detection via available WiFi networks
Required By     : None
Optional For    : None
Conflicts With  : None
Replaces        : None
Installed Size  : 257.63 MiB
Packager        : Levente Polyak <anthraxx@archlinux.org>
Build Date      : Tue 14 May 2024 05:00:00 PM UTC
Install Date    : Sun 19 May 2024 09:12:44 AM UTC
Install Reason  : Explicitly installed
Install Script  : No
Validated By    : Signature

Name            : bash
Version         : 5.2.026-2
Description     : The GNU Bourne Again shell
Groups          : base
Installed Size  : 8.62 MiB
Packager        : Unknown Packager
Install Date    : Mon 01 Jan 2024 10:00:00 AM UTC
Install Reason  : Explicitly installed
";

    #[test]
    fn pacman_parses_blocks_with_multiline_fields() {
        let v = parse_pacman(PACMAN);
        assert_eq!(v.len(), 2);
        let ff = &v[0].entry;
        assert_eq!(ff.id, "pacman:firefox");
        assert_eq!(ff.version, "126.0-1");
        assert_eq!(ff.publisher, "Levente Polyak");
        assert_eq!(ff.install_date.as_deref(), Some("2024-05-19"));
        assert_eq!(ff.size_bytes, Some((257.63 * 1024.0 * 1024.0) as u64));
        assert!(!ff.is_system);
        let bash = &v[1].entry;
        assert!(bash.is_system);
        assert_eq!(bash.publisher, "");
        assert!(parse_pacman("").is_empty());
        assert!(parse_pacman("\n\n\n").is_empty());
    }

    /// `flatpak list --app --columns=application,name,version,size,origin` (piped).
    const FLATPAK: &str = "\
org.mozilla.firefox\tFirefox\t126.0\t278.5\u{a0}MB\tflathub
org.gimp.GIMP\tGIMP\t2.10.36\t1.2\u{a0}GB\tflathub
com.example.NoVersion\tNo Version\t\t12.0\u{a0}kB\tfedora
";

    #[test]
    fn flatpak_parses_tabs_nbsp_sizes_and_skips_header() {
        let with_header = format!("Application ID\tName\tVersion\tSize\tOrigin\n{FLATPAK}");
        let v = parse_flatpak(&with_header);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].entry.id, "flatpak:org.mozilla.firefox");
        assert_eq!(v[0].entry.name, "Firefox");
        assert_eq!(v[0].entry.version, "126.0");
        assert_eq!(v[0].entry.size_bytes, Some(278_500_000));
        assert_eq!(v[0].entry.publisher, "flathub");
        assert_eq!(v[1].entry.size_bytes, Some(1_200_000_000));
        assert_eq!(v[2].entry.version, "");
        assert_eq!(v[2].entry.size_bytes, Some(12_000));
        assert!(parse_flatpak("").is_empty());
        assert!(parse_flatpak("no apps\n").is_empty());
    }

    /// `snap list` on Ubuntu 24.04.
    const SNAP: &str = "\
Name                       Version           Rev    Tracking         Publisher      Notes
bare                       1.0               5      latest/stable    canonical**    base
core22                     20240111          1380   latest/stable    canonical**    base
firefox                    126.0-2           4336   latest/stable/…  mozilla**      -
gnome-42-2204              0+git.ff35a85     176    latest/stable/…  canonical**    -
snapd                      2.63              21759  latest/stable    canonical**    snapd
mything                    1.0               x1     -                -              -
";

    #[test]
    fn snap_parses_and_flags_bases() {
        let v = parse_snap(SNAP);
        let names: Vec<&str> = v.iter().map(|f| f.entry.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "bare",
                "core22",
                "firefox",
                "gnome-42-2204",
                "snapd",
                "mything"
            ]
        );
        assert!(v[0].entry.is_system && v[1].entry.is_system && v[4].entry.is_system);
        assert!(!v[2].entry.is_system && !v[3].entry.is_system);
        assert_eq!(v[2].entry.publisher, "mozilla");
        assert_eq!(v[2].entry.version, "126.0-2");
        assert_eq!(v[5].entry.publisher, "");
        assert!(parse_snap("").is_empty());
        assert!(parse_snap("Name Version Rev Tracking Publisher Notes\n").is_empty());
        // "No snaps are installed yet." style messages
        assert!(
            parse_snap("No snaps are installed yet. Try 'snap install hello-world'.\n").is_empty()
        );
    }

    #[test]
    fn appimage_names() {
        assert_eq!(
            split_appimage_name("Obsidian-1.5.3"),
            ("Obsidian".into(), "1.5.3".into())
        );
        assert_eq!(
            split_appimage_name("Foo-Bar-v2.0.1-x86_64"),
            ("Foo Bar".into(), "2.0.1".into())
        );
        assert_eq!(
            split_appimage_name("kdenlive-24.02.1-x86_64"),
            ("kdenlive".into(), "24.02.1".into())
        );
        assert_eq!(
            split_appimage_name("Plain"),
            ("Plain".into(), String::new())
        );
        assert_eq!(
            split_appimage_name("app_2024.1"),
            ("app".into(), "2024.1".into())
        );
        assert_eq!(
            split_appimage_name("balenaEtcher-1.18.11-x64"),
            ("balenaEtcher".into(), "1.18.11-x64".into())
        );
    }

    #[test]
    fn appimages_are_found_in_both_folders_and_symlinks_ignored() {
        let (_d, c) = ctx();
        let apps = c.env.home.join("Applications");
        let bin = c.env.home.join(".local/bin");
        fs::create_dir_all(&apps).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::write(apps.join("Obsidian-1.5.3.AppImage"), vec![0u8; 2048]).unwrap();
        fs::write(bin.join("tool.appimage"), "x").unwrap();
        fs::write(bin.join("notes.txt"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            apps.join("Obsidian-1.5.3.AppImage"),
            bin.join("link.AppImage"),
        )
        .unwrap();
        let v = scan_appimages(&c);
        assert_eq!(v.len(), 2);
        let ob = v.iter().find(|f| f.entry.name == "Obsidian").unwrap();
        assert_eq!(ob.entry.version, "1.5.3");
        assert_eq!(ob.entry.size_bytes, Some(2048));
        assert_eq!(ob.entry.source, Source::Appimage);
        assert!(ob.entry.id.starts_with("appimage:/"));
        assert_eq!(
            appimage_path_of(&ob.entry.id),
            Some(apps.join("Obsidian-1.5.3.AppImage").as_path())
        );
        assert!(matches!(ob.action, Action::AppImage(_)));
    }

    #[test]
    fn brew_list_versions() {
        let v = parse_brew("wget 1.24.5\ngit 2.44.0 2.45.1\nffmpeg 7.0\n", false);
        assert_eq!(v.len(), 3);
        assert_eq!(v[1].entry.version, "2.45.1");
        assert_eq!(v[0].entry.id, "brew:wget");
        let c = parse_brew("firefox 126.0\niterm2 3.5.2\n", true);
        assert_eq!(c[0].entry.id, "brew-cask:firefox");
        assert_eq!(
            c[0].action,
            Action::Brew {
                name: "firefox".into(),
                cask: true
            }
        );
        assert!(parse_brew("", true).is_empty());
        assert!(parse_brew("--flag 1\n", true).is_empty());
    }
}
