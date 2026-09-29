//! Windows registry categories. Every scanner is conservative: an entry is only reported when
//! the file it needs is unambiguously identified and definitely gone (see [`super::cmdline`]).

use crate::error::{ApiError, Result};
use crate::job::Job;

use super::cmdline::{
    expand_env, locate_command, locate_path, strip_resource_index, windir, Located,
};
use super::model::{Action, Found, Severity};
use super::regaccess::{RegValue, RegistryAccess, Root, Stat};

// ---------------------------------------------------------------- helpers

/// Text that can be handed to `reg.exe` unchanged.
pub fn reg_text_ok(s: &str) -> bool {
    !s.contains(['"', '%']) && !s.chars().any(char::is_control)
}

/// A value name `reg delete /v` can address safely ("" = the default value).
pub fn value_name_ok(s: &str) -> bool {
    s.is_empty() || (reg_text_ok(s) && !s.starts_with(['/', '-']))
}

/// A key path `reg delete` can address safely.
pub fn key_path_ok(s: &str) -> bool {
    !s.is_empty()
        && reg_text_ok(s)
        && !s.starts_with(['/', '-', '\\'])
        && !s.ends_with('\\')
        && !s.contains("\\\\")
}

fn join(a: &str, b: &str) -> String {
    format!("{a}\\{b}")
}

fn tick(job: &Job, i: usize) -> Result<()> {
    if i.is_multiple_of(128) {
        job.check_cancelled()?;
    }
    Ok(())
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "..."
    }
}

fn del_value(
    cat: &str,
    description: String,
    root: Root,
    key: &str,
    name: &str,
    data: Option<String>,
    severity: Severity,
) -> Option<Found> {
    if !key_path_ok(key) || !value_name_ok(name) {
        return None;
    }
    Some(Found::registry(
        cat,
        description,
        root,
        key,
        Some(name),
        data,
        severity,
        Action::RegDeleteValue {
            root,
            key: key.to_string(),
            name: name.to_string(),
        },
    ))
}

fn del_key(
    cat: &str,
    description: String,
    root: Root,
    key: &str,
    data: Option<String>,
    severity: Severity,
) -> Option<Found> {
    if !key_path_ok(key) {
        return None;
    }
    Some(Found::registry(
        cat,
        description,
        root,
        key,
        None,
        data,
        severity,
        Action::RegDeleteKey {
            root,
            key: key.to_string(),
        },
    ))
}

fn default_str(vals: &[RegValue]) -> Option<String> {
    vals.iter()
        .find(|v| v.name.is_empty())
        .and_then(|v| v.data.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn is_guid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 38
        && b[0] == b'{'
        && b[37] == b'}'
        && s[1..37].char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

const CLASSES: &str = r"SOFTWARE\Classes";
const CLASSES_WOW: &str = r"SOFTWARE\Classes\WOW6432Node";
const CV: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion";
const CV_WOW: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion";

// ---------------------------------------------------------------- entry point

/// Scan one category (id from [`super::model::WINDOWS_CATEGORIES`]).
pub fn scan_category(reg: &dyn RegistryAccess, id: &str, job: &Job) -> Result<Vec<Found>> {
    job.check_cancelled()?;
    match id {
        "shared_dlls" => shared_dlls(reg),
        "file_extensions" => file_extensions(reg, job),
        "activex" => activex(reg, job),
        "type_libs" => type_libs(reg, job),
        "applications" => applications(reg, job),
        "fonts" => fonts(reg),
        "app_paths" => app_paths(reg, job),
        "help_files" => help_files(reg),
        "installer" => installer(reg),
        "obsolete_software" => obsolete_software(reg, job),
        "startup" => startup(reg),
        "menu_order" => menu_order(reg),
        "mui_cache" => mui_cache(reg, job),
        "sound_events" => sound_events(reg, job),
        "services" => services(reg, job),
        other => Err(ApiError::invalid_params(format!(
            "unknown registry category `{other}`"
        ))),
    }
}

// ---------------------------------------------------------------- categories

fn shared_dlls(reg: &dyn RegistryAccess) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    for key in [format!(r"{CV}\SharedDLLs"), format!(r"{CV_WOW}\SharedDLLs")] {
        for v in reg.values(Root::Hklm, &key) {
            if v.name.is_empty() {
                continue;
            }
            if let Located::Missing(p) = locate_path(reg, &v.name) {
                out.extend(del_value(
                    "shared_dlls",
                    format!("Shared file not found: {p}"),
                    Root::Hklm,
                    &key,
                    &v.name,
                    Some(v.data.display()).filter(|d| !d.is_empty()),
                    Severity::Low,
                ));
            }
        }
    }
    Ok(out)
}

/// Sub-keys of an extension key that carry no information of their own.
fn only_empty_openwith(reg: &dyn RegistryAccess, root: Root, key: &str, subs: &[String]) -> bool {
    subs.iter().all(|s| {
        let k = join(key, s);
        s.eq_ignore_ascii_case("OpenWithProgids")
            && reg.values(root, &k).is_empty()
            && reg.subkeys(root, &k).is_empty()
    })
}

fn file_extensions(reg: &dyn RegistryAccess, job: &Job) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    for root in [Root::Hklm, Root::Hkcu] {
        for (i, name) in reg.subkeys(root, CLASSES).into_iter().enumerate() {
            tick(job, i)?;
            if !name.starts_with('.') || name.len() < 2 || name.len() > 40 {
                continue;
            }
            let key = join(CLASSES, &name);
            let vals = reg.values(root, &key);
            let subs = reg.subkeys(root, &key);
            // Any value besides the default (Content Type, PerceivedType, ...) means the
            // extension is doing something.
            if vals.iter().any(|v| !v.name.is_empty()) {
                continue;
            }
            if !only_empty_openwith(reg, root, &key, &subs) {
                continue;
            }
            let progid = default_str(&vals);
            let description = match progid {
                None => format!("The file type {name} has no program associated with it"),
                Some(p) => {
                    let exists = [Root::Hklm, Root::Hkcu]
                        .into_iter()
                        .any(|r| reg.key_exists(r, &join(CLASSES, &p)));
                    if exists {
                        continue;
                    }
                    format!(
                        "The file type {name} points to a program type ({p}) that does not exist"
                    )
                }
            };
            out.extend(del_key(
                "file_extensions",
                description,
                root,
                &key,
                default_str(&vals),
                Severity::Low,
            ));
        }
    }
    Ok(out)
}

fn activex(reg: &dyn RegistryAccess, job: &Job) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    let mut n = 0usize;
    for root in [Root::Hklm, Root::Hkcu] {
        for base in [format!(r"{CLASSES}\CLSID"), format!(r"{CLASSES_WOW}\CLSID")] {
            for clsid in reg.subkeys(root, &base) {
                n += 1;
                tick(job, n)?;
                if !is_guid(&clsid) {
                    continue;
                }
                let key = join(&base, &clsid);
                let subs = reg.subkeys(root, &key);
                let has = |name: &str| subs.iter().any(|s| s.eq_ignore_ascii_case(name));
                // Redirections and handlers mean the class can still work.
                if has("TreatAs")
                    || has("AutoTreatAs")
                    || has("AutoConvertTo")
                    || has("InprocHandler32")
                    || has("InprocHandler")
                {
                    continue;
                }
                let servers: Vec<&String> = subs
                    .iter()
                    .filter(|s| {
                        s.eq_ignore_ascii_case("InprocServer32")
                            || s.eq_ignore_ascii_case("LocalServer32")
                    })
                    .collect();
                if servers.is_empty() {
                    continue;
                }
                let mut missing_path = None;
                let mut all_missing = true;
                for s in servers {
                    let vals = reg.values(root, &join(&key, s));
                    let Some(cmd) = default_str(&vals) else {
                        all_missing = false;
                        break;
                    };
                    match locate_command(reg, &cmd) {
                        Located::Missing(p) => missing_path = Some(p),
                        _ => {
                            all_missing = false;
                            break;
                        }
                    }
                }
                if !all_missing {
                    continue;
                }
                let Some(p) = missing_path else { continue };
                let friendly = default_str(&reg.values(root, &key)).unwrap_or_default();
                let label = if friendly.is_empty() {
                    clsid.clone()
                } else {
                    format!("{} {}", short(&friendly, 60), clsid)
                };
                out.extend(del_key(
                    "activex",
                    format!("The component {label} needs a file that no longer exists: {p}"),
                    root,
                    &key,
                    None,
                    Severity::Low,
                ));
            }
        }
    }
    Ok(out)
}

fn type_libs(reg: &dyn RegistryAccess, job: &Job) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    let mut n = 0usize;
    for root in [Root::Hklm, Root::Hkcu] {
        for base in [
            format!(r"{CLASSES}\TypeLib"),
            format!(r"{CLASSES_WOW}\TypeLib"),
        ] {
            for guid in reg.subkeys(root, &base) {
                if !is_guid(&guid) {
                    continue;
                }
                let gkey = join(&base, &guid);
                for ver in reg.subkeys(root, &gkey) {
                    n += 1;
                    tick(job, n)?;
                    let vkey = join(&gkey, &ver);
                    let mut targets: Vec<Located> = Vec::new();
                    for lcid in reg.subkeys(root, &vkey) {
                        // FLAGS and HELPDIR are not locale keys
                        if lcid.eq_ignore_ascii_case("FLAGS")
                            || lcid.eq_ignore_ascii_case("HELPDIR")
                        {
                            continue;
                        }
                        let lkey = join(&vkey, &lcid);
                        for plat in reg.subkeys(root, &lkey) {
                            if !(plat.eq_ignore_ascii_case("win32")
                                || plat.eq_ignore_ascii_case("win64"))
                            {
                                continue;
                            }
                            let Some(p) = default_str(&reg.values(root, &join(&lkey, &plat)))
                            else {
                                targets.push(Located::Unknown);
                                continue;
                            };
                            targets.push(locate_path(reg, strip_resource_index(&p)));
                        }
                    }
                    if targets.is_empty() || !targets.iter().all(Located::is_missing) {
                        continue;
                    }
                    let Some(Located::Missing(p)) = targets.into_iter().next() else {
                        continue;
                    };
                    out.extend(del_key(
                        "type_libs",
                        format!("Type library {guid} version {ver} needs a file that no longer exists: {p}"),
                        root,
                        &vkey,
                        None,
                        Severity::Low,
                    ));
                }
            }
        }
    }
    Ok(out)
}

fn applications(reg: &dyn RegistryAccess, job: &Job) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    for root in [Root::Hklm, Root::Hkcu] {
        let base = format!(r"{CLASSES}\Applications");
        for (i, app) in reg.subkeys(root, &base).into_iter().enumerate() {
            tick(job, i)?;
            let key = join(&base, &app);
            let shell = join(&key, "shell");
            let verbs = reg.subkeys(root, &shell);
            if verbs.is_empty() {
                continue;
            }
            // Every verb must be broken: an `open` that is gone but an `edit` that works
            // means the entry is still in use.
            let mut first = None;
            let mut all = true;
            for verb in &verbs {
                let cmd = default_str(&reg.values(root, &join(&join(&shell, verb), "command")));
                match cmd.map(|c| locate_command(reg, &c)) {
                    Some(Located::Missing(p)) => first = first.or(Some(p)),
                    _ => {
                        all = false;
                        break;
                    }
                }
            }
            if !all {
                continue;
            }
            let Some(p) = first else { continue };
            out.extend(del_key(
                "applications",
                format!("The program entry {app} points to a program that no longer exists: {p}"),
                root,
                &key,
                None,
                Severity::Low,
            ));
        }
    }
    Ok(out)
}

fn fonts(reg: &dyn RegistryAccess) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    let Some(w) = windir(reg) else {
        return Ok(out);
    };
    let fonts_dir = format!(r"{w}\Fonts");
    let user_fonts = reg
        .env_var("LOCALAPPDATA")
        .map(|l| format!(r"{}\Microsoft\Windows\Fonts", l.trim_end_matches('\\')));
    for root in [Root::Hklm, Root::Hkcu] {
        let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts";
        for v in reg.values(root, key) {
            let Some(data) = v.data.as_str().map(str::trim).filter(|d| !d.is_empty()) else {
                continue;
            };
            let data = expand_env(reg, data);
            let absolute = data.as_bytes().get(1) == Some(&b':') || data.starts_with("\\\\");
            let verdict = if absolute {
                locate_path(reg, &data)
            } else if data.contains(['\\', '/', ';', ',']) {
                Located::Unknown
            } else {
                // a bare file name: installed fonts live in the Fonts folder (or the per-user one)
                let mut verdicts = vec![locate_path(reg, &format!(r"{fonts_dir}\{data}"))];
                if let Some(u) = &user_fonts {
                    verdicts.push(locate_path(reg, &format!(r"{u}\{data}")));
                }
                if verdicts.iter().all(Located::is_missing) {
                    verdicts.swap_remove(0)
                } else {
                    Located::Unknown
                }
            };
            if let Located::Missing(p) = verdict {
                out.extend(del_value(
                    "fonts",
                    format!(
                        "The font {} is registered but its file is missing: {p}",
                        short(&v.name, 80)
                    ),
                    root,
                    key,
                    &v.name,
                    Some(data.clone()),
                    Severity::Low,
                ));
            }
        }
    }
    Ok(out)
}

fn app_paths(reg: &dyn RegistryAccess, job: &Job) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    for (root, base) in [
        (Root::Hklm, format!(r"{CV}\App Paths")),
        (Root::Hklm, format!(r"{CV_WOW}\App Paths")),
        (Root::Hkcu, format!(r"{CV}\App Paths")),
    ] {
        for (i, app) in reg.subkeys(root, &base).into_iter().enumerate() {
            tick(job, i)?;
            let key = join(&base, &app);
            let Some(cmd) = default_str(&reg.values(root, &key)) else {
                continue;
            };
            if let Located::Missing(p) = locate_command(reg, &cmd) {
                out.extend(del_key(
                    "app_paths",
                    format!("The program location for {app} is missing: {p}"),
                    root,
                    &key,
                    Some(cmd),
                    Severity::Low,
                ));
            }
        }
    }
    Ok(out)
}

fn help_files(reg: &dyn RegistryAccess) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    for key in [
        r"SOFTWARE\Microsoft\Windows\Help",
        r"SOFTWARE\Microsoft\Windows\HTML Help",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\Help",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\HTML Help",
    ] {
        for v in reg.values(Root::Hklm, key) {
            let Some(dir) = v.data.as_str().map(str::trim).filter(|d| !d.is_empty()) else {
                continue;
            };
            let name = v.name.trim();
            if name.is_empty() {
                continue;
            }
            let dir = expand_env(reg, dir);
            let lower = dir.to_ascii_lowercase();
            let path = if name.as_bytes().get(1) == Some(&b':') {
                name.to_string()
            } else if lower.ends_with(".hlp") || lower.ends_with(".chm") || lower.ends_with(".cnt")
            {
                dir.clone()
            } else {
                format!(r"{}\{}", dir.trim_end_matches('\\'), name)
            };
            if let Located::Missing(p) = locate_path(reg, &path) {
                out.extend(del_value(
                    "help_files",
                    format!("The help file {name} is missing: {p}"),
                    Root::Hklm,
                    key,
                    &v.name,
                    Some(dir),
                    Severity::Low,
                ));
            }
        }
    }
    Ok(out)
}

fn installer(reg: &dyn RegistryAccess) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    let key = format!(r"{CV}\Installer\Folders");
    for v in reg.values(Root::Hklm, &key) {
        // Only real drive paths: the key also holds bare tokens on some systems.
        if v.name.as_bytes().get(1) != Some(&b':') {
            continue;
        }
        if let Located::Missing(p) = locate_path(reg, &v.name) {
            out.extend(del_value(
                "installer",
                format!("The installer folder no longer exists: {p}"),
                Root::Hklm,
                &key,
                &v.name,
                None,
                Severity::Low,
            ));
        }
    }
    Ok(out)
}

const UPDATE_TYPES: &[&str] = &[
    "update",
    "hotfix",
    "security update",
    "update rollup",
    "service pack",
    "critical update",
];

fn dword_of(vals: &[RegValue], name: &str) -> Option<u32> {
    vals.iter()
        .find(|v| v.name.eq_ignore_ascii_case(name))
        .and_then(|v| v.data.as_dword())
}

fn str_of(vals: &[RegValue], name: &str) -> Option<String> {
    vals.iter()
        .find(|v| v.name.eq_ignore_ascii_case(name))
        .and_then(|v| v.data.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn obsolete_software(reg: &dyn RegistryAccess, job: &Job) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    for (root, base) in [
        (Root::Hklm, format!(r"{CV}\Uninstall")),
        (Root::Hklm, format!(r"{CV_WOW}\Uninstall")),
        (Root::Hkcu, format!(r"{CV}\Uninstall")),
    ] {
        for (i, sub) in reg.subkeys(root, &base).into_iter().enumerate() {
            tick(job, i)?;
            let key = join(&base, &sub);
            let vals = reg.values(root, &key);
            let Some(name) = str_of(&vals, "DisplayName") else {
                continue;
            };
            if dword_of(&vals, "SystemComponent") == Some(1)
                || dword_of(&vals, "WindowsInstaller") == Some(1)
                || str_of(&vals, "ParentKeyName").is_some()
                || str_of(&vals, "ParentDisplayName").is_some()
            {
                continue;
            }
            if let Some(rt) = str_of(&vals, "ReleaseType") {
                if UPDATE_TYPES.contains(&rt.to_ascii_lowercase().as_str()) {
                    continue;
                }
            }
            // Every uninstaller the entry names must be gone.
            let uninstallers: Vec<String> = ["UninstallString", "QuietUninstallString"]
                .iter()
                .filter_map(|n| str_of(&vals, n))
                .collect();
            if uninstallers.is_empty() {
                continue;
            }
            let mut first_missing = None;
            let mut all_missing = true;
            for u in &uninstallers {
                match locate_command(reg, u) {
                    Located::Missing(p) => first_missing = first_missing.or(Some(p)),
                    _ => {
                        all_missing = false;
                        break;
                    }
                }
            }
            if !all_missing {
                continue;
            }
            // ...and so must the install location and the icon file, when there are any.
            if let Some(loc) = str_of(&vals, "InstallLocation") {
                if !locate_path(reg, &loc).is_missing() {
                    continue;
                }
            }
            if let Some(icon) = str_of(&vals, "DisplayIcon") {
                let icon = icon.trim_matches('"');
                let icon = icon.rsplit_once(',').map_or(icon, |(p, idx)| {
                    if idx
                        .trim()
                        .trim_start_matches('-')
                        .bytes()
                        .all(|b| b.is_ascii_digit())
                    {
                        p
                    } else {
                        icon
                    }
                });
                if !locate_path(reg, icon).is_missing() {
                    continue;
                }
            }
            let Some(p) = first_missing else { continue };
            out.extend(del_key(
                "obsolete_software",
                format!(
                    "{} is no longer installed (its uninstaller is missing: {p})",
                    short(&name, 80)
                ),
                root,
                &key,
                None,
                Severity::Medium,
            ));
        }
    }
    Ok(out)
}

fn startup(reg: &dyn RegistryAccess) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    for (root, key) in [
        (Root::Hklm, format!(r"{CV}\Run")),
        (Root::Hklm, format!(r"{CV_WOW}\Run")),
        (Root::Hkcu, format!(r"{CV}\Run")),
    ] {
        for v in reg.values(root, &key) {
            let Some(cmd) = v.data.as_str() else { continue };
            if v.name.is_empty() {
                continue;
            }
            if let Located::Missing(p) = locate_command(reg, cmd) {
                out.extend(del_value(
                    "startup",
                    format!(
                        "{} is set to start with Windows but the program is missing: {p}",
                        short(&v.name, 60)
                    ),
                    root,
                    &key,
                    &v.name,
                    Some(cmd.trim().to_string()),
                    Severity::Low,
                ));
            }
        }
    }
    Ok(out)
}

fn menu_order(reg: &dyn RegistryAccess) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    let (Some(appdata), Some(progdata)) = (reg.env_var("APPDATA"), reg.env_var("ProgramData"))
    else {
        return Ok(out);
    };
    let user_programs = format!(
        r"{}\Microsoft\Windows\Start Menu\Programs",
        appdata.trim_end_matches('\\')
    );
    let common_programs = format!(
        r"{}\Microsoft\Windows\Start Menu\Programs",
        progdata.trim_end_matches('\\')
    );
    // Both Start menu folders must be readable, otherwise nothing can be concluded.
    if reg.stat(&user_programs) != Stat::Dir || reg.stat(&common_programs) != Stat::Dir {
        return Ok(out);
    }
    for menu in ["Start Menu2", "Start Menu"] {
        let base = format!(
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\MenuOrder\{menu}\Programs"
        );
        for folder in reg.subkeys(Root::Hkcu, &base) {
            let stats = [
                reg.stat(&format!(r"{user_programs}\{folder}")),
                reg.stat(&format!(r"{common_programs}\{folder}")),
            ];
            if stats.iter().all(|s| *s == Stat::Missing) {
                out.extend(del_key(
                    "menu_order",
                    format!(
                        "Saved sort order for the Start menu folder \"{}\", which no longer exists",
                        short(&folder, 60)
                    ),
                    Root::Hkcu,
                    &join(&base, &folder),
                    None,
                    Severity::Low,
                ));
            }
        }
    }
    Ok(out)
}

fn mui_cache(reg: &dyn RegistryAccess, job: &Job) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    let key = r"SOFTWARE\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache";
    for (i, v) in reg.values(Root::Hkcu, key).into_iter().enumerate() {
        tick(job, i)?;
        let lower = v.name.to_ascii_lowercase();
        let Some(suffix) = [".friendlyappname", ".applicationcompany"]
            .into_iter()
            .find(|s| lower.ends_with(s))
        else {
            continue;
        };
        let path = &v.name[..v.name.len() - suffix.len()];
        if path.as_bytes().get(1) != Some(&b':') {
            continue; // @shell32.dll,-123 resource strings and the like
        }
        if let Located::Missing(p) = locate_path(reg, path) {
            out.extend(del_value(
                "mui_cache",
                format!("Cached name for a program that no longer exists: {p}"),
                Root::Hkcu,
                key,
                &v.name,
                v.data.as_str().map(|s| short(s, 80)),
                Severity::Low,
            ));
        }
    }
    Ok(out)
}

fn sound_events(reg: &dyn RegistryAccess, job: &Job) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    let base = r"AppEvents\Schemes\Apps";
    let media = windir(reg).map(|w| format!(r"{w}\Media"));
    for (i, app) in reg.subkeys(Root::Hkcu, base).into_iter().enumerate() {
        tick(job, i)?;
        let akey = join(base, &app);
        for event in reg.subkeys(Root::Hkcu, &akey) {
            let key = join(&join(&akey, &event), ".Current");
            let Some(file) = default_str(&reg.values(Root::Hkcu, &key)) else {
                continue;
            };
            let target = if !file.contains(['\\', '/']) && !file.contains('%') {
                match &media {
                    Some(m) => format!(r"{m}\{file}"),
                    None => continue,
                }
            } else {
                file.clone()
            };
            if let Located::Missing(p) = locate_path(reg, &target) {
                out.extend(del_value(
                    "sound_events",
                    format!(
                        "The sound for \"{}\" ({}) is missing: {p}",
                        short(&event, 40),
                        short(&app, 40)
                    ),
                    Root::Hkcu,
                    &key,
                    "",
                    Some(file),
                    Severity::Low,
                ));
            }
        }
    }
    Ok(out)
}

fn services(reg: &dyn RegistryAccess, job: &Job) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    let base = r"SYSTEM\CurrentControlSet\Services";
    let w = windir(reg).map(|w| w.to_ascii_lowercase());
    for (i, svc) in reg.subkeys(Root::Hklm, base).into_iter().enumerate() {
        tick(job, i)?;
        let key = join(base, &svc);
        let vals = reg.values(Root::Hklm, &key);
        // Drivers (kernel 1, file system 2, adapter 4, recognizer 8) are never touched: only
        // services that run as their own or a shared process.
        let Some(t) = dword_of(&vals, "Type") else {
            continue;
        };
        if t & 0x30 == 0 || t & 0x0f != 0 {
            continue;
        }
        let Some(image) = str_of(&vals, "ImagePath") else {
            continue;
        };
        if let Located::Missing(p) = locate_command(reg, &image) {
            // Anything inside the Windows directory is part of the operating system (optional
            // features that were removed leave such entries): leave those alone.
            if w.as_ref()
                .is_some_and(|w| p.to_ascii_lowercase().starts_with(w.as_str()))
            {
                continue;
            }
            out.extend(del_key(
                "services",
                format!(
                    "The service {} runs a program that no longer exists: {p}",
                    short(&svc, 60)
                ),
                Root::Hklm,
                &key,
                Some(image),
                Severity::Medium,
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::fake::FakeWin;
    use super::super::regaccess::RegData;
    use super::*;

    fn scan(w: &FakeWin, id: &str) -> Vec<Found> {
        scan_category(w, id, &Job::detached()).unwrap()
    }

    fn descs(f: &[Found]) -> Vec<String> {
        f.iter()
            .map(|x| x.issue.location.clone() + "|" + x.issue.value.as_deref().unwrap_or(""))
            .collect()
    }

    #[test]
    fn reg_argument_safety() {
        assert!(value_name_ok(""));
        assert!(value_name_ok(r"C:\Program Files\a.dll"));
        assert!(value_name_ok(r"C:\Program Files\Foo\"));
        assert!(!value_name_ok("a\"b"));
        assert!(!value_name_ok("50%"));
        assert!(!value_name_ok("/f"));
        assert!(!value_name_ok("-v"));
        assert!(!value_name_ok("a\nb"));
        assert!(key_path_ok(r"SOFTWARE\A\B"));
        assert!(!key_path_ok(""));
        assert!(!key_path_ok(r"A\B\"));
        assert!(!key_path_ok(r"A\\B"));
        assert!(!key_path_ok("-x"));
        assert!(!key_path_ok(r"A\%B%"));
    }

    #[test]
    fn unknown_category_is_rejected() {
        let w = FakeWin::new();
        assert!(scan_category(&w, "nope", &Job::detached()).is_err());
    }

    #[test]
    fn cancelled_scan_stops() {
        let w = FakeWin::new();
        let job = Job::detached();
        job.token().cancel();
        assert!(scan_category(&w, "shared_dlls", &job).is_err());
    }

    // ---------------- shared dlls

    #[test]
    fn shared_dlls() {
        let w = FakeWin::new();
        w.file(r"C:\Program Files\Common Files\ok.dll")
            .file(r"C:\Windows\SysWOW64\wow.dll");
        let k = r"SOFTWARE\Microsoft\Windows\CurrentVersion\SharedDLLs";
        w.dword(Root::Hklm, k, r"C:\Program Files\Common Files\ok.dll", 2);
        w.dword(Root::Hklm, k, r"C:\Program Files\Common Files\gone.dll", 1);
        w.dword(Root::Hklm, k, r"C:\Windows\System32\wow.dll", 1); // redirected
        w.dword(Root::Hklm, k, r"E:\usb\x.dll", 1); // unreadable drive
        w.dword(Root::Hklm, k, r"%NOPE%\x.dll", 1);
        w.dword(Root::Hklm, k, "", 1);
        w.dword(
            Root::Hklm,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\SharedDLLs",
            r"C:\Program Files (x86)\Common Files\gone32.dll",
            1,
        );
        let f = scan(&w, "shared_dlls");
        assert_eq!(f.len(), 2, "{:?}", descs(&f));
        assert!(f[0].issue.location.ends_with("SharedDLLs"));
        assert_eq!(
            f[0].issue.value.as_deref(),
            Some(r"C:\Program Files\Common Files\gone.dll")
        );
        assert_eq!(f[0].issue.data.as_deref(), Some("1"));
        assert!(f[0].issue.needs_admin);
        assert!(f[1].issue.location.contains("WOW6432Node"));
        assert!(matches!(
            f[0].action,
            Action::RegDeleteValue {
                root: Root::Hklm,
                ..
            }
        ));
    }

    // ---------------- file extensions

    #[test]
    fn file_extensions() {
        let w = FakeWin::new();
        let c = r"SOFTWARE\Classes";
        // dangling ProgID -> reported
        w.sz(Root::Hklm, &format!(r"{c}\.abc"), "", "abcfile");
        // ProgID exists -> fine
        w.sz(Root::Hklm, &format!(r"{c}\.txt"), "", "txtfile");
        w.key(Root::Hklm, &format!(r"{c}\txtfile"));
        // ProgID exists only in HKCU -> fine
        w.sz(Root::Hklm, &format!(r"{c}\.usr"), "", "usrfile");
        w.key(Root::Hkcu, &format!(r"{c}\usrfile"));
        // empty key -> reported
        w.key(Root::Hkcu, &format!(r"{c}\.empty"));
        // MIME-only extension -> left alone
        w.sz(
            Root::Hklm,
            &format!(r"{c}\.mime"),
            "Content Type",
            "text/x-foo",
        );
        // dangling ProgID but has a shell verb -> left alone
        w.sz(Root::Hklm, &format!(r"{c}\.shl"), "", "missingprog");
        w.key(Root::Hklm, &format!(r"{c}\.shl\shell"));
        // dangling ProgID with an empty OpenWithProgids -> reported
        w.sz(Root::Hklm, &format!(r"{c}\.owp"), "", "missingprog2");
        w.key(Root::Hklm, &format!(r"{c}\.owp\OpenWithProgids"));
        // OpenWithProgids with content -> left alone
        w.sz(Root::Hklm, &format!(r"{c}\.owq"), "", "missingprog3");
        w.set(
            Root::Hklm,
            &format!(r"{c}\.owq\OpenWithProgids"),
            "x.File",
            RegData::Other,
        );
        // not extensions
        w.key(Root::Hklm, &format!(r"{c}\notdot"));
        w.key(Root::Hklm, &format!(r"{c}\."));
        let f = scan(&w, "file_extensions");
        let mut locs: Vec<_> = f.iter().map(|x| x.issue.location.clone()).collect();
        locs.sort();
        assert_eq!(
            locs,
            [
                r"HKCU\SOFTWARE\Classes\.empty",
                r"HKLM\SOFTWARE\Classes\.abc",
                r"HKLM\SOFTWARE\Classes\.owp"
            ]
        );
        assert!(f
            .iter()
            .all(|x| matches!(x.action, Action::RegDeleteKey { .. })));
        let empty = f
            .iter()
            .find(|x| x.issue.location.ends_with(".empty"))
            .unwrap();
        assert!(!empty.issue.needs_admin);
    }

    // ---------------- activex

    fn clsid(
        w: &FakeWin,
        root: Root,
        base: &str,
        guid: &str,
        inproc: Option<&str>,
        local: Option<&str>,
    ) {
        let k = format!(r"{base}\{guid}");
        w.sz(root, &k, "", "Friendly Thing");
        if let Some(d) = inproc {
            w.sz(root, &format!(r"{k}\InprocServer32"), "", d);
            w.sz(
                root,
                &format!(r"{k}\InprocServer32"),
                "ThreadingModel",
                "Apartment",
            );
        }
        if let Some(d) = local {
            w.sz(root, &format!(r"{k}\LocalServer32"), "", d);
        }
    }

    const G1: &str = "{11111111-1111-1111-1111-111111111111}";
    const G2: &str = "{22222222-2222-2222-2222-222222222222}";
    const G3: &str = "{33333333-3333-3333-3333-333333333333}";
    const G4: &str = "{44444444-4444-4444-4444-444444444444}";
    const G5: &str = "{55555555-5555-5555-5555-555555555555}";
    const G6: &str = "{66666666-6666-6666-6666-666666666666}";

    #[test]
    fn activex() {
        let w = FakeWin::new();
        w.file(r"C:\Program Files\Acme\ok.dll")
            .file(r"C:\Program Files\Acme\ok.exe");
        let base = r"SOFTWARE\Classes\CLSID";
        clsid(
            &w,
            Root::Hklm,
            base,
            G1,
            Some(r"C:\Program Files\Acme\gone.dll"),
            None,
        ); // reported
        clsid(
            &w,
            Root::Hklm,
            base,
            G2,
            Some(r"C:\Program Files\Acme\ok.dll"),
            None,
        ); // fine
           // one server present, the other gone -> fine
        clsid(
            &w,
            Root::Hklm,
            base,
            G3,
            Some(r"C:\Program Files\Acme\gone.dll"),
            Some(r"C:\Program Files\Acme\ok.exe"),
        );
        // both gone -> reported
        clsid(
            &w,
            Root::Hklm,
            base,
            G4,
            Some(r"C:\Program Files\Acme\gone.dll"),
            Some(r#""C:\Program Files\Acme\gone.exe" /Embedding"#),
        );
        // quoted args + env var
        clsid(
            &w,
            Root::Hkcu,
            base,
            G5,
            None,
            Some(r#""%ProgramFiles%\Acme\gone.exe" -Embedding"#),
        ); // reported (HKCU)
           // empty default (Win11 classic menu trick) -> fine
        clsid(&w, Root::Hkcu, base, G6, Some(""), None);
        // bare .NET shim resolved from system32
        w.file(r"C:\Windows\System32\mscoree.dll");
        clsid(
            &w,
            Root::Hklm,
            base,
            "{77777777-7777-7777-7777-777777777777}",
            Some("mscoree.dll"),
            None,
        );
        // WOW6432 view + redirect
        w.file(r"C:\Windows\SysWOW64\wow.dll");
        clsid(
            &w,
            Root::Hklm,
            r"SOFTWARE\Classes\WOW6432Node\CLSID",
            "{88888888-8888-8888-8888-888888888888}",
            Some(r"C:\Windows\System32\wow.dll"),
            None,
        );
        clsid(
            &w,
            Root::Hklm,
            r"SOFTWARE\Classes\WOW6432Node\CLSID",
            "{99999999-9999-9999-9999-999999999999}",
            Some(r"C:\Windows\System32\gone32.dll"),
            None,
        ); // reported
           // TreatAs redirect -> fine even though the server is gone
        clsid(
            &w,
            Root::Hklm,
            base,
            "{AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA}",
            Some(r"C:\Program Files\Acme\gone.dll"),
            None,
        );
        w.key(
            Root::Hklm,
            &format!(r"{base}\{{AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA}}\TreatAs"),
        );
        // not a guid
        clsid(&w, Root::Hklm, base, "NotAGuid", Some(r"C:\gone.dll"), None);
        let f = scan(&w, "activex");
        let mut locs: Vec<_> = f.iter().map(|x| x.issue.location.clone()).collect();
        locs.sort();
        assert_eq!(
            locs,
            [
                format!(r"HKCU\SOFTWARE\Classes\CLSID\{G5}"),
                format!(r"HKLM\SOFTWARE\Classes\CLSID\{G1}"),
                format!(r"HKLM\SOFTWARE\Classes\CLSID\{G4}"),
                r"HKLM\SOFTWARE\Classes\WOW6432Node\CLSID\{99999999-9999-9999-9999-999999999999}"
                    .to_string(),
            ]
        );
        assert!(f[0].issue.description.contains("Friendly Thing"));
    }

    // ---------------- type libs

    fn typelib(w: &FakeWin, root: Root, guid: &str, ver: &str, lcid: &str, plat: &str, path: &str) {
        w.sz(
            root,
            &format!(r"SOFTWARE\Classes\TypeLib\{guid}\{ver}\{lcid}\{plat}"),
            "",
            path,
        );
    }

    #[test]
    fn type_libs() {
        let w = FakeWin::new();
        w.file(r"C:\Program Files\Acme\ok.tlb")
            .file(r"C:\Program Files\Acme\ok.dll");
        typelib(
            &w,
            Root::Hklm,
            G1,
            "1.0",
            "0",
            "win32",
            r"C:\Program Files\Acme\gone.tlb",
        ); // reported
        typelib(
            &w,
            Root::Hklm,
            G2,
            "1.0",
            "0",
            "win32",
            r"C:\Program Files\Acme\ok.tlb",
        ); // fine
        typelib(
            &w,
            Root::Hklm,
            G3,
            "2.0",
            "409",
            "win32",
            r"C:\Program Files\Acme\ok.dll\2",
        ); // resource index, fine
        typelib(
            &w,
            Root::Hklm,
            G4,
            "1.1",
            "0",
            "win32",
            r"C:\Program Files\Acme\gone.dll\3",
        ); // reported
           // win32 gone but win64 present -> fine
        typelib(
            &w,
            Root::Hklm,
            G5,
            "1.0",
            "0",
            "win32",
            r"C:\Program Files\Acme\gone.tlb",
        );
        typelib(
            &w,
            Root::Hklm,
            G5,
            "1.0",
            "0",
            "win64",
            r"C:\Program Files\Acme\ok.tlb",
        );
        // two locales, one present -> fine
        typelib(
            &w,
            Root::Hkcu,
            G6,
            "1.0",
            "0",
            "win32",
            r"C:\Program Files\Acme\gone.tlb",
        );
        typelib(
            &w,
            Root::Hkcu,
            G6,
            "1.0",
            "409",
            "win32",
            r"C:\Program Files\Acme\ok.tlb",
        );
        // FLAGS / HELPDIR are not locales
        w.sz(
            Root::Hklm,
            &format!(r"SOFTWARE\Classes\TypeLib\{G1}\1.0\HELPDIR"),
            "",
            r"C:\gone",
        );
        // second version of G1 is fine and stays
        typelib(
            &w,
            Root::Hklm,
            G1,
            "2.0",
            "0",
            "win32",
            r"C:\Program Files\Acme\ok.tlb",
        );
        // missing default -> unknown -> not reported
        w.key(
            Root::Hklm,
            &format!(r"SOFTWARE\Classes\TypeLib\{G2}\9.9\0\win32"),
        );
        let f = scan(&w, "type_libs");
        let mut locs: Vec<_> = f.iter().map(|x| x.issue.location.clone()).collect();
        locs.sort();
        assert_eq!(
            locs,
            [
                format!(r"HKLM\SOFTWARE\Classes\TypeLib\{G1}\1.0"),
                format!(r"HKLM\SOFTWARE\Classes\TypeLib\{G4}\1.1"),
            ]
        );
    }

    // ---------------- applications

    #[test]
    fn applications() {
        let w = FakeWin::new();
        w.file(r"C:\Program Files\Acme\ok.exe");
        let b = r"SOFTWARE\Classes\Applications";
        w.sz(
            Root::Hklm,
            &format!(r"{b}\gone.exe\shell\open\command"),
            "",
            r#""C:\Program Files\Acme\gone.exe" "%1""#,
        );
        w.sz(
            Root::Hklm,
            &format!(r"{b}\ok.exe\shell\open\command"),
            "",
            r#""C:\Program Files\Acme\ok.exe" "%1""#,
        );
        // open gone, edit works -> keep
        w.sz(
            Root::Hkcu,
            &format!(r"{b}\mixed.exe\shell\open\command"),
            "",
            r#""C:\Program Files\Acme\gone.exe" "%1""#,
        );
        w.sz(
            Root::Hkcu,
            &format!(r"{b}\mixed.exe\shell\edit\command"),
            "",
            r#""C:\Program Files\Acme\ok.exe" "%1""#,
        );
        // no shell verbs -> keep
        w.key(Root::Hkcu, &format!(r"{b}\noshell.exe"));
        // command is just %1 -> unknown
        w.sz(
            Root::Hkcu,
            &format!(r"{b}\weird.exe\shell\open\command"),
            "",
            "%1",
        );
        let f = scan(&w, "applications");
        assert_eq!(f.len(), 1);
        assert_eq!(
            f[0].issue.location,
            r"HKLM\SOFTWARE\Classes\Applications\gone.exe"
        );
    }

    // ---------------- fonts

    #[test]
    fn fonts() {
        let w = FakeWin::new();
        w.file(r"C:\Windows\Fonts\arial.ttf")
            .file(r"C:\Users\Bob\AppData\Local\Microsoft\Windows\Fonts\mine.ttf");
        w.file(r"C:\Custom\abs.ttf");
        let k = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts";
        w.sz(Root::Hklm, k, "Arial (TrueType)", "arial.ttf");
        w.sz(Root::Hklm, k, "Gone (TrueType)", "gone.ttf"); // reported
        w.sz(Root::Hklm, k, "Mine (TrueType)", "mine.ttf"); // per-user folder
        w.sz(Root::Hklm, k, "Abs (TrueType)", r"C:\Custom\abs.ttf");
        w.sz(
            Root::Hklm,
            k,
            "AbsGone (TrueType)",
            r"C:\Custom\absgone.ttf",
        ); // reported
        w.sz(
            Root::Hkcu,
            k,
            "UserGone (TrueType)",
            r"C:\Users\Bob\AppData\Local\Microsoft\Windows\Fonts\ug.ttf",
        ); // reported
        w.sz(Root::Hklm, k, "Odd", "sub\\dir.ttf"); // ambiguous
        w.sz(Root::Hklm, k, "Empty", "");
        w.sz(Root::Hklm, k, "Net", r"\\srv\share\f.ttf");
        w.expand(
            Root::Hklm,
            k,
            "Env (TrueType)",
            r"%SystemRoot%\Fonts\arial.ttf",
        );
        w.expand(
            Root::Hklm,
            k,
            "EnvGone (TrueType)",
            r"%SystemRoot%\Fonts\envgone.ttf",
        ); // reported
        let f = scan(&w, "fonts");
        let mut vals: Vec<_> = f.iter().map(|x| x.issue.value.clone().unwrap()).collect();
        vals.sort();
        assert_eq!(
            vals,
            [
                "AbsGone (TrueType)",
                "EnvGone (TrueType)",
                "Gone (TrueType)",
                "UserGone (TrueType)"
            ]
        );
    }

    // ---------------- app paths

    #[test]
    fn app_paths() {
        let w = FakeWin::new();
        w.file(r"C:\Program Files\Acme\ok.exe");
        let b = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";
        w.sz(
            Root::Hklm,
            &format!(r"{b}\ok.exe"),
            "",
            r"C:\Program Files\Acme\ok.exe",
        );
        w.sz(
            Root::Hklm,
            &format!(r"{b}\gone.exe"),
            "",
            r#""C:\Program Files\Acme\gone.exe""#,
        ); // reported
        w.sz(
            Root::Hkcu,
            &format!(r"{b}\ugone.exe"),
            "",
            r"C:\Program Files\Acme\ugone.exe",
        ); // reported
        w.sz(
            Root::Hklm,
            &format!(r"{b}\onlypath.exe"),
            "Path",
            r"C:\gone",
        );
        w.key(Root::Hklm, &format!(r"{b}\onlypath.exe"));
        w.sz(
            Root::Hklm,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths\w.exe",
            "",
            r"C:\Program Files (x86)\Acme\w.exe",
        ); // reported
        w.sz(Root::Hklm, &format!(r"{b}\bare.exe"), "", "notthere.exe"); // bare: unknown
        let f = scan(&w, "app_paths");
        assert_eq!(f.len(), 3, "{:?}", descs(&f));
        assert!(f
            .iter()
            .all(|x| matches!(x.action, Action::RegDeleteKey { .. })));
    }

    // ---------------- help files

    #[test]
    fn help_files() {
        let w = FakeWin::new();
        w.file(r"C:\Program Files\Acme\ok.chm")
            .file(r"C:\Program Files\Acme\ok.hlp");
        w.sz(
            Root::Hklm,
            r"SOFTWARE\Microsoft\Windows\HTML Help",
            "ok.chm",
            r"C:\Program Files\Acme\",
        );
        w.sz(
            Root::Hklm,
            r"SOFTWARE\Microsoft\Windows\HTML Help",
            "gone.chm",
            r"C:\Program Files\Acme\",
        ); // reported
        w.sz(
            Root::Hklm,
            r"SOFTWARE\Microsoft\Windows\Help",
            "ok.hlp",
            r"C:\Program Files\Acme",
        );
        w.sz(
            Root::Hklm,
            r"SOFTWARE\Microsoft\Windows\Help",
            "gone.hlp",
            r"C:\Program Files\Acme",
        ); // reported
        w.sz(
            Root::Hklm,
            r"SOFTWARE\Microsoft\Windows\Help",
            "x.hlp",
            r"C:\Program Files\Acme\gone2.hlp",
        ); // data is the file: reported
        w.sz(
            Root::Hklm,
            r"SOFTWARE\Microsoft\Windows\Help",
            "y.hlp",
            r"E:\usb",
        ); // removable: unknown
        let f = scan(&w, "help_files");
        let mut v: Vec<_> = f.iter().map(|x| x.issue.value.clone().unwrap()).collect();
        v.sort();
        assert_eq!(v, ["gone.chm", "gone.hlp", "x.hlp"]);
    }

    // ---------------- installer folders

    #[test]
    fn installer_folders() {
        let w = FakeWin::new();
        w.dir(r"C:\Program Files\Acme");
        let k = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\Folders";
        w.sz(Root::Hklm, k, r"C:\Program Files\Acme\", "");
        w.sz(Root::Hklm, k, r"C:\Program Files\Gone\", ""); // reported
        w.sz(Root::Hklm, k, "TARGETDIR", "");
        let f = scan(&w, "installer");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].issue.value.as_deref(), Some(r"C:\Program Files\Gone\"));
    }

    // ---------------- obsolete software

    fn uninstall(w: &FakeWin, root: Root, base: &str, key: &str, name: &str, us: &str) {
        let k = format!(r"{base}\{key}");
        w.sz(root, &k, "DisplayName", name);
        w.sz(root, &k, "UninstallString", us);
    }

    #[test]
    fn obsolete_software() {
        let w = FakeWin::new();
        w.file(r"C:\Program Files\Acme\unins000.exe")
            .file(r"C:\Program Files\Acme\acme.exe");
        w.file(r"C:\Windows\System32\msiexec.exe");
        let b = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
        uninstall(
            &w,
            Root::Hklm,
            b,
            "Gone",
            "Gone App",
            r#""C:\Program Files\Gone\unins000.exe" /SILENT"#,
        ); // reported
        uninstall(
            &w,
            Root::Hklm,
            b,
            "Acme",
            "Acme",
            r#""C:\Program Files\Acme\unins000.exe""#,
        ); // installed
           // uninstaller gone but install location exists -> keep
        uninstall(
            &w,
            Root::Hklm,
            b,
            "Loc",
            "Loc App",
            r"C:\Program Files\Nope\u.exe",
        );
        w.sz(
            Root::Hklm,
            &format!(r"{b}\Loc"),
            "InstallLocation",
            r"C:\Program Files\Acme",
        );
        // uninstaller gone, install location gone too -> reported
        uninstall(
            &w,
            Root::Hklm,
            b,
            "Loc2",
            "Loc2 App",
            r"C:\Program Files\Nope\u.exe",
        );
        w.sz(
            Root::Hklm,
            &format!(r"{b}\Loc2"),
            "InstallLocation",
            r"C:\Program Files\Nope",
        );
        // display icon still exists -> keep
        uninstall(
            &w,
            Root::Hklm,
            b,
            "Icon",
            "Icon App",
            r"C:\Program Files\Nope\u.exe",
        );
        w.sz(
            Root::Hklm,
            &format!(r"{b}\Icon"),
            "DisplayIcon",
            r"C:\Program Files\Acme\acme.exe,0",
        );
        // MSI product: msiexec exists -> keep
        uninstall(
            &w,
            Root::Hklm,
            b,
            "{90160000-008C-0000-0000-0000000FF1CE}",
            "Office",
            "MsiExec.exe /X{90160000-008C-0000-0000-0000000FF1CE}",
        );
        // WindowsInstaller flag with a gone uninstaller -> still skipped
        uninstall(
            &w,
            Root::Hklm,
            b,
            "{A0160000-008C-0000-0000-0000000FF1CE}",
            "MSI2",
            r"C:\gone\x.exe",
        );
        w.dword(
            Root::Hklm,
            &format!(r"{b}\{{A0160000-008C-0000-0000-0000000FF1CE}}"),
            "WindowsInstaller",
            1,
        );
        // update / system component / child
        uninstall(&w, Root::Hklm, b, "KB1", "Update", r"C:\gone\kb.exe");
        w.sz(
            Root::Hklm,
            &format!(r"{b}\KB1"),
            "ReleaseType",
            "Security Update",
        );
        uninstall(&w, Root::Hklm, b, "Sys", "Sys", r"C:\gone\s.exe");
        w.dword(Root::Hklm, &format!(r"{b}\Sys"), "SystemComponent", 1);
        uninstall(&w, Root::Hklm, b, "Child", "Child", r"C:\gone\c.exe");
        w.sz(
            Root::Hklm,
            &format!(r"{b}\Child"),
            "ParentKeyName",
            "Parent",
        );
        // no display name / no uninstaller
        w.sz(
            Root::Hklm,
            &format!(r"{b}\NoName"),
            "UninstallString",
            r"C:\gone\n.exe",
        );
        w.sz(
            Root::Hklm,
            &format!(r"{b}\NoUn"),
            "DisplayName",
            "No uninstall string",
        );
        // ClickOnce style
        uninstall(
            &w,
            Root::Hkcu,
            b,
            "Click",
            "Click App",
            "rundll32.exe dfshim.dll,ShArpMaintain App.application",
        );
        w.file(r"C:\Windows\System32\dfshim.dll");
        // unresolvable variable -> unknown
        uninstall(&w, Root::Hkcu, b, "Var", "Var App", r"%NOPE%\u.exe");
        // both uninstall strings: quiet one still exists -> keep
        uninstall(&w, Root::Hkcu, b, "Quiet", "Quiet App", r"C:\gone\u.exe");
        w.sz(
            Root::Hkcu,
            &format!(r"{b}\Quiet"),
            "QuietUninstallString",
            r"C:\Program Files\Acme\unins000.exe /S",
        );
        let f = scan(&w, "obsolete_software");
        let mut keys: Vec<_> = f
            .iter()
            .map(|x| x.issue.location.rsplit('\\').next().unwrap().to_string())
            .collect();
        keys.sort();
        assert_eq!(keys, ["Gone", "Loc2"]);
        assert!(f.iter().all(|x| x.issue.severity == Severity::Medium));
    }

    // ---------------- startup

    #[test]
    fn startup() {
        let w = FakeWin::new();
        w.file(r"C:\Program Files\Acme\ok.exe");
        let k = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
        w.sz(
            Root::Hklm,
            k,
            "Ok",
            r#""C:\Program Files\Acme\ok.exe" --tray"#,
        );
        w.sz(
            Root::Hklm,
            k,
            "Gone",
            r#""C:\Program Files\Acme\gone.exe" --tray"#,
        ); // reported
        w.sz(
            Root::Hkcu,
            k,
            "UserGone",
            r"C:\Program Files\Acme\ugone.exe /min",
        ); // reported
        w.expand(Root::Hkcu, k, "EnvGone", r"%ProgramFiles%\Acme\egone.exe"); // reported
        w.sz(Root::Hkcu, k, "Cmd", r"cmd.exe /c C:\gone\x.bat"); // launcher present
        w.sz(Root::Hkcu, k, "Bare", "someprogram.exe"); // unknown
        w.sz(
            Root::Hkcu,
            k,
            "Rundll",
            r#"rundll32.exe "C:\Program Files\Acme\gone.dll",Run"#,
        ); // reported
        w.dword(Root::Hkcu, k, "Dword", 1);
        w.sz(
            Root::Hklm,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run",
            "W",
            r"C:\Program Files (x86)\Acme\ok.exe",
        ); // WOW alt: present
        let f = scan(&w, "startup");
        let mut v: Vec<_> = f.iter().map(|x| x.issue.value.clone().unwrap()).collect();
        v.sort();
        assert_eq!(v, ["EnvGone", "Gone", "Rundll", "UserGone"]);
        let ug = f
            .iter()
            .find(|x| x.issue.value.as_deref() == Some("UserGone"))
            .unwrap();
        assert!(!ug.issue.needs_admin);
    }

    // ---------------- menu order

    #[test]
    fn menu_order() {
        let w = FakeWin::new();
        w.dir(r"C:\Users\Bob\AppData\Roaming\Microsoft\Windows\Start Menu\Programs");
        w.dir(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs");
        w.dir(r"C:\Users\Bob\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Mine");
        w.dir(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Common");
        let b =
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\MenuOrder\Start Menu2\Programs";
        for name in ["Mine", "Common", "Gone"] {
            w.key(Root::Hkcu, &format!(r"{b}\{name}"));
        }
        let f = scan(&w, "menu_order");
        assert_eq!(f.len(), 1);
        assert!(f[0].issue.location.ends_with(r"\Gone"));
        assert_eq!(f[0].issue.severity, Severity::Low);
        // an unreadable Start menu folder disables the category
        w.unreadable(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs");
        assert!(scan(&w, "menu_order").is_empty());
    }

    // ---------------- mui cache

    #[test]
    fn mui_cache() {
        let w = FakeWin::new();
        w.file(r"C:\Program Files\Acme\ok.exe");
        let k = r"SOFTWARE\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache";
        w.sz(
            Root::Hkcu,
            k,
            r"C:\Program Files\Acme\ok.exe.FriendlyAppName",
            "Ok",
        );
        w.sz(
            Root::Hkcu,
            k,
            r"C:\Program Files\Acme\gone.exe.FriendlyAppName",
            "Gone",
        ); // reported
        w.sz(
            Root::Hkcu,
            k,
            r"C:\Program Files\Acme\gone.exe.ApplicationCompany",
            "Acme",
        ); // reported
        w.sz(Root::Hkcu, k, "@shell32.dll,-21770", "Documents");
        w.dword(Root::Hkcu, k, "LangID", 1033);
        w.sz(Root::Hkcu, k, r"C:\Program Files\Acme\other.txt", "x");
        let f = scan(&w, "mui_cache");
        assert_eq!(f.len(), 2);
        assert!(f.iter().all(|x| !x.issue.needs_admin));
    }

    // ---------------- sound events

    #[test]
    fn sound_events() {
        let w = FakeWin::new();
        w.file(r"C:\Windows\Media\ding.wav");
        let b = r"AppEvents\Schemes\Apps";
        w.sz(
            Root::Hkcu,
            &format!(r"{b}\.Default\Notify\.Current"),
            "",
            r"C:\Windows\media\ding.wav",
        );
        w.sz(
            Root::Hkcu,
            &format!(r"{b}\.Default\Gone\.Current"),
            "",
            r"C:\Windows\media\gone.wav",
        ); // reported
        w.sz(
            Root::Hkcu,
            &format!(r"{b}\.Default\Env\.Current"),
            "",
            r"%SystemRoot%\media\envgone.wav",
        ); // reported
        w.sz(
            Root::Hkcu,
            &format!(r"{b}\.Default\Rel\.Current"),
            "",
            "ding.wav",
        ); // relative, present
        w.sz(
            Root::Hkcu,
            &format!(r"{b}\.Default\RelGone\.Current"),
            "",
            "relgone.wav",
        ); // reported
        w.sz(Root::Hkcu, &format!(r"{b}\.Default\Empty\.Current"), "", "");
        w.sz(
            Root::Hkcu,
            &format!(r"{b}\.Default\Gone\.Default"),
            "",
            r"C:\gone.wav",
        ); // only .Current counts
        let f = scan(&w, "sound_events");
        assert_eq!(f.len(), 3, "{:?}", descs(&f));
        for x in &f {
            assert!(matches!(&x.action, Action::RegDeleteValue { name, .. } if name.is_empty()));
            assert_eq!(x.issue.value.as_deref(), Some("(Default)"));
        }
    }

    // ---------------- services

    fn service(w: &FakeWin, name: &str, ty: u32, image: &str) {
        let k = format!(r"SYSTEM\CurrentControlSet\Services\{name}");
        w.dword(Root::Hklm, &k, "Type", ty);
        w.expand(Root::Hklm, &k, "ImagePath", image);
    }

    #[test]
    fn services() {
        let w = FakeWin::new();
        w.file(r"C:\Program Files\Acme\svc.exe")
            .file(r"C:\Windows\System32\svchost.exe");
        service(
            &w,
            "AcmeGone",
            0x10,
            r#""C:\Program Files\Acme\gone.exe" -service"#,
        ); // reported
        service(&w, "AcmeOk", 0x10, r#""C:\Program Files\Acme\svc.exe""#);
        service(&w, "AcmeShared", 0x20, r"C:\Program Files\Acme\gone2.exe"); // reported (shared process)
        service(&w, "Driver", 0x1, r"C:\Program Files\Acme\gone.sys"); // driver
        service(&w, "FsDriver", 0x2, r"C:\Program Files\Acme\gone.sys");
        service(
            &w,
            "Host",
            0x20,
            r"%SystemRoot%\system32\svchost.exe -k netsvcs",
        );
        service(
            &w,
            "OsGone",
            0x10,
            r"%SystemRoot%\system32\removedfeature.exe",
        ); // inside Windows dir: skipped
        service(&w, "OsGone2", 0x10, r"\SystemRoot\System32\removed2.exe"); // skipped
        service(&w, "Interactive", 0x110, r"C:\Program Files\Acme\gone3.exe"); // reported
        service(&w, "Rel", 0x10, r"system32\thing.exe"); // relative: unknown
        w.dword(
            Root::Hklm,
            r"SYSTEM\CurrentControlSet\Services\NoImage",
            "Type",
            0x10,
        );
        let f = scan(&w, "services");
        let mut v: Vec<_> = f
            .iter()
            .map(|x| x.issue.location.rsplit('\\').next().unwrap().to_string())
            .collect();
        v.sort();
        assert_eq!(v, ["AcmeGone", "AcmeShared", "Interactive"]);
        assert!(f.iter().all(|x| x.issue.severity == Severity::Medium));
    }
}
