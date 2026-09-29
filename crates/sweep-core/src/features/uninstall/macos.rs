//! macOS applications: `.app` bundles in `/Applications` and `~/Applications`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ctx::Ctx;
use crate::elevate::applescript_escape;
use crate::features::uninstall::model::{Action, Found, Source};
use crate::pkgutil::{date_from_unix, path_size};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BundleInfo {
    pub name: String,
    pub version: String,
    pub bundle_id: String,
}

/// Read the interesting keys from an `Info.plist` (XML or binary).
pub fn parse_info_plist(bytes: &[u8]) -> Option<BundleInfo> {
    let v = plist::Value::from_reader(std::io::Cursor::new(bytes)).ok()?;
    let d = v.as_dictionary()?;
    let s = |k: &str| {
        d.get(k)
            .and_then(|v| v.as_string())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    Some(BundleInfo {
        name: s("CFBundleDisplayName")
            .or_else(|| s("CFBundleName"))
            .unwrap_or_default(),
        version: s("CFBundleShortVersionString")
            .or_else(|| s("CFBundleVersion"))
            .unwrap_or_default(),
        bundle_id: s("CFBundleIdentifier").unwrap_or_default(),
    })
}

/// The bundle id of a listed macOS app id (`macapp:<path>`) is not stored in the id, so
/// leftover scans read it from the bundle when it still exists; otherwise from `id`.
pub fn bundle_id_of(app: &Path) -> Option<String> {
    let bytes = fs::read(app.join("Contents").join("Info.plist")).ok()?;
    let info = parse_info_plist(&bytes)?;
    (!info.bundle_id.is_empty()).then_some(info.bundle_id)
}

fn is_apple(bundle_id: &str) -> bool {
    bundle_id.starts_with("com.apple.")
}

/// The `.app` folders directly inside `/Applications` and `~/Applications`.
pub fn app_dirs(ctx: &Ctx) -> [PathBuf; 2] {
    [
        ctx.env.sys_path("/Applications"),
        ctx.env.home.join("Applications"),
    ]
}

pub fn scan_apps(ctx: &Ctx, sizes: bool) -> Vec<Found> {
    let mut v = Vec::new();
    for dir in app_dirs(ctx) {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let fname = e.file_name().to_string_lossy().into_owned();
            let Some(stem) = fname.strip_suffix(".app") else {
                continue;
            };
            let path = e.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.file_type().is_dir() {
                continue;
            }
            let info = fs::read(path.join("Contents").join("Info.plist"))
                .ok()
                .and_then(|b| parse_info_plist(&b))
                .unwrap_or_default();
            let name = if info.name.is_empty() {
                stem.to_string()
            } else {
                info.name.clone()
            };
            let mut f = Found::new(
                format!("macapp:{}", path.display()),
                name,
                info.version.clone(),
                Source::Macapp,
                Action::MacApp(path.clone()),
            );
            f.entry.size_bytes = sizes.then(|| path_size(&path));
            f.entry.install_date = meta
                .created()
                .or_else(|_| meta.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| date_from_unix(d.as_secs() as i64));
            f.entry.is_system = is_apple(&info.bundle_id);
            v.push(f);
        }
    }
    v
}

/// AppleScript that moves `path` to the Trash through Finder (which also handles the
/// authorization prompt for root-owned bundles).
pub fn trash_script(path: &Path) -> String {
    format!(
        "tell application \"Finder\" to delete POSIX file \"{}\"",
        applescript_escape(&path.to_string_lossy())
    )
}

/// Check `path` is a real (non-symlink) `.app` directly inside one of the application folders.
pub fn validate_app_path(ctx: &Ctx, path: &Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| n.ends_with(".app") && n.len() > 4)
        .ok_or("not an application bundle")?;
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.file_type().is_dir() {
        return Err("application bundle is not a folder (or is a link)".into());
    }
    let parent = path.parent().ok_or("no parent folder")?;
    let parent = crate::safety::canonicalize(parent).map_err(|e| e.to_string())?;
    for dir in app_dirs(ctx) {
        if let Ok(d) = crate::safety::canonicalize(&dir) {
            if d == parent {
                return Ok(parent.join(name));
            }
        }
    }
    Err("application is not inside /Applications or ~/Applications".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::MockRunner;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleIdentifier</key>
	<string>org.mozilla.firefox</string>
	<key>CFBundleName</key>
	<string>Firefox</string>
	<key>CFBundleShortVersionString</key>
	<string>126.0</string>
	<key>CFBundleVersion</key>
	<string>12600.1.2</string>
</dict>
</plist>
"#;

    fn ctx() -> (tempfile::TempDir, Ctx) {
        let d = tempfile::tempdir().unwrap();
        let mut c = Ctx::test(d.path(), MockRunner::new());
        c.env.os = crate::ctx::Os::MacOs;
        fs::create_dir_all(&c.env.home).unwrap();
        (d, c)
    }

    fn make_app(dir: &Path, stem: &str, plist: Option<&[u8]>) -> PathBuf {
        let app = dir.join(format!("{stem}.app"));
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(app.join("Contents/MacOS/bin"), vec![0u8; 1000]).unwrap();
        if let Some(p) = plist {
            fs::write(app.join("Contents/Info.plist"), p).unwrap();
        }
        app
    }

    #[test]
    fn info_plist_xml_and_binary() {
        let i = parse_info_plist(XML.as_bytes()).unwrap();
        assert_eq!(i.name, "Firefox");
        assert_eq!(i.version, "126.0");
        assert_eq!(i.bundle_id, "org.mozilla.firefox");

        let mut dict = plist::Dictionary::new();
        dict.insert("CFBundleDisplayName".into(), "Display Name".into());
        dict.insert("CFBundleName".into(), "Name".into());
        dict.insert("CFBundleVersion".into(), "42".into());
        let mut bin = Vec::new();
        plist::Value::Dictionary(dict)
            .to_writer_binary(&mut bin)
            .unwrap();
        let i = parse_info_plist(&bin).unwrap();
        assert_eq!(i.name, "Display Name");
        assert_eq!(i.version, "42");
        assert_eq!(i.bundle_id, "");

        assert!(parse_info_plist(b"not a plist").is_none());
        assert!(parse_info_plist(b"").is_none());
    }

    #[test]
    fn scans_both_application_folders() {
        let (_d, c) = ctx();
        let sys = c.env.sys_path("/Applications");
        let usr = c.env.home.join("Applications");
        fs::create_dir_all(&sys).unwrap();
        fs::create_dir_all(&usr).unwrap();
        make_app(&sys, "Firefox", Some(XML.as_bytes()));
        make_app(&sys, "NoPlist", None);
        let safari = XML
            .replace("org.mozilla.firefox", "com.apple.Safari")
            .replace("Firefox", "Safari");
        make_app(&sys, "Safari", Some(safari.as_bytes()));
        make_app(&usr, "Mine", None);
        fs::write(sys.join("readme.txt"), "x").unwrap();
        fs::create_dir_all(sys.join("Utilities")).unwrap();
        let v = scan_apps(&c, true);
        let names: Vec<&str> = v.iter().map(|f| f.entry.name.as_str()).collect();
        assert_eq!(names, ["Firefox", "NoPlist", "Safari", "Mine"]);
        assert_eq!(v[0].entry.version, "126.0");
        assert_eq!(v[0].entry.size_bytes.map(|s| s >= 1000), Some(true));
        assert!(!v[0].entry.is_system && v[2].entry.is_system);
        assert!(v[0].entry.id.starts_with("macapp:"));
        assert!(v[0].entry.id.ends_with("/Applications/Firefox.app"));
        assert_eq!(v[1].entry.version, "");
        assert_eq!(
            bundle_id_of(&sys.join("Firefox.app")).as_deref(),
            Some("org.mozilla.firefox")
        );
        assert_eq!(bundle_id_of(&sys.join("NoPlist.app")), None);
    }

    #[test]
    fn trash_script_escapes_paths() {
        assert_eq!(
            trash_script(Path::new("/Applications/My \"Cool\" App.app")),
            "tell application \"Finder\" to delete POSIX file \"/Applications/My \\\"Cool\\\" App.app\""
        );
    }

    #[test]
    fn app_path_validation() {
        let (_d, c) = ctx();
        let sys = c.env.sys_path("/Applications");
        fs::create_dir_all(&sys).unwrap();
        let app = make_app(&sys, "Firefox", None);
        assert!(validate_app_path(&c, &app).is_ok());
        // wrong extension, nested, outside, missing
        assert!(validate_app_path(&c, &sys.join("Firefox")).is_err());
        assert!(validate_app_path(&c, &app.join("Contents/MacOS")).is_err());
        let other = c.env.home.join("Downloads");
        fs::create_dir_all(&other).unwrap();
        let outside = make_app(&other, "Evil", None);
        assert!(validate_app_path(&c, &outside).is_err());
        assert!(validate_app_path(&c, &sys.join("Missing.app")).is_err());
        assert!(validate_app_path(&c, Path::new("/")).is_err());
        #[cfg(unix)]
        {
            let link = sys.join("Link.app");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            assert!(validate_app_path(&c, &link).is_err());
        }
    }
}
