//! Cleaning rules: ClearSweep's own declarative rule files (see `rules/README.md`).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::ctx::Os;
use crate::features::cleaner::template;

const FILES: &[(&str, &str)] = &[
    ("browsers.toml", include_str!("rules/browsers.toml")),
    ("system_linux.toml", include_str!("rules/system_linux.toml")),
    (
        "system_windows.toml",
        include_str!("rules/system_windows.toml"),
    ),
    ("system_macos.toml", include_str!("rules/system_macos.toml")),
    ("apps.toml", include_str!("rules/apps.toml")),
];

/// Operating system names as written in rule files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsName {
    Linux,
    Windows,
    Macos,
}

impl OsName {
    pub fn from_os(os: Os) -> OsName {
        match os {
            Os::Linux => OsName::Linux,
            Os::Windows => OsName::Windows,
            Os::MacOs => OsName::Macos,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            OsName::Linux => "linux",
            OsName::Windows => "windows",
            OsName::Macos => "macos",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Browser,
    System,
    Application,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    Chromium,
    Firefox,
}

/// Names of processes that mean the app is running, per OS.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Processes {
    #[serde(default)]
    pub linux: Vec<String>,
    #[serde(default)]
    pub windows: Vec<String>,
    #[serde(default)]
    pub macos: Vec<String>,
}

impl Processes {
    pub fn for_os(&self, os: Os) -> &[String] {
        match os {
            Os::Linux => &self.linux,
            Os::Windows => &self.windows,
            Os::MacOs => &self.macos,
        }
    }
}

fn yes() -> bool {
    true
}
fn star() -> String {
    "*".to_string()
}

/// Delete files under `base` that match `pattern`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesTarget {
    #[serde(default)]
    pub os: Vec<OsName>,
    /// Path template. Ignored when `literal_base` is set.
    #[serde(default)]
    pub base: String,
    /// Absolute directory from validated user settings (custom include entries only);
    /// never read from rule files.
    #[serde(skip)]
    pub literal_base: Option<PathBuf>,
    /// Glob matched against file names.
    #[serde(default = "star")]
    pub pattern: String,
    #[serde(default)]
    pub recursive: bool,
    /// Only files at least this old.
    #[serde(default)]
    pub min_age_hours: Option<u32>,
    /// Use `tempMinAgeHours` from settings as the minimum age.
    #[serde(default)]
    pub min_age_from_settings: bool,
    /// Remove directories that end up empty (respecting the age filter).
    #[serde(default)]
    pub remove_empty_dirs: bool,
    /// Keep the base directory itself (default). When false it is removed if empty.
    #[serde(default = "yes")]
    pub keep_base: bool,
    /// Only entries owned by the current user (Unix; no-op elsewhere).
    #[serde(default)]
    pub owned_by_user: bool,
    /// Entry names (globs) never touched at any depth; matching directories are skipped whole.
    #[serde(default)]
    pub skip_names: Vec<String>,
}

/// Delete individual files (globs allowed in the template).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileTarget {
    #[serde(default)]
    pub os: Vec<OsName>,
    pub path: String,
}

/// Row-level cleaning of a SQLite database.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SqliteTarget {
    #[serde(default)]
    pub os: Vec<OsName>,
    pub db: String,
    pub statements: Vec<String>,
    /// Single-value query reporting the rows the statements will remove.
    pub count_query: String,
}

/// Cookie database cleaning honouring the keep list.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CookiesTarget {
    #[serde(default)]
    pub os: Vec<OsName>,
    pub browser_family: Family,
    pub db: String,
}

/// Run a program (only if it exists).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandTarget {
    #[serde(default)]
    pub os: Vec<OsName>,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Shown as the action name; defaults to the program name.
    #[serde(default)]
    pub label: Option<String>,
}

/// The OS trash / recycle bin.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrashTarget {
    #[serde(default)]
    pub os: Vec<OsName>,
}

/// Windows registry values / subkeys under an allow-listed HKCU key.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistryTarget {
    #[serde(default)]
    pub os: Vec<OsName>,
    pub key: String,
    #[serde(default)]
    pub values: bool,
    #[serde(default)]
    pub subkeys: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Target {
    Files(FilesTarget),
    File(FileTarget),
    Sqlite(SqliteTarget),
    Cookies(CookiesTarget),
    Command(CommandTarget),
    Trash(TrashTarget),
    Registry(RegistryTarget),
}

impl Target {
    /// OS restriction of this target (empty = every OS of the rule).
    pub fn os_list(&self) -> &[OsName] {
        match self {
            Target::Files(t) => &t.os,
            Target::File(t) => &t.os,
            Target::Sqlite(t) => &t.os,
            Target::Cookies(t) => &t.os,
            Target::Command(t) => &t.os,
            Target::Trash(t) => &t.os,
            Target::Registry(t) => &t.os,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub group: String,
    pub category: Category,
    pub os: Vec<OsName>,
    #[serde(default)]
    pub default_enabled: bool,
    pub description: String,
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(default)]
    pub processes: Processes,
    pub targets: Vec<Target>,
}

impl Rule {
    pub fn applies_to(&self, os: Os) -> bool {
        self.os.contains(&OsName::from_os(os))
    }
    /// Targets that apply on `os`.
    pub fn targets_for(&self, os: Os) -> impl Iterator<Item = &Target> {
        let name = OsName::from_os(os);
        self.targets.iter().filter(move |t| {
            let l = t.os_list();
            if l.is_empty() {
                self.os.contains(&name)
            } else {
                l.contains(&name)
            }
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    #[serde(default)]
    rule: Vec<Rule>,
}

fn parse_all() -> Result<Vec<Rule>, String> {
    let mut all = Vec::new();
    for (name, text) in FILES {
        let f: RuleFile = toml::from_str(text).map_err(|e| format!("{name}: {e}"))?;
        all.extend(f.rule);
    }
    Ok(all)
}

/// Every embedded rule (all operating systems). Panics if an embedded file is malformed;
/// the unit tests parse and validate them all so this cannot ship broken.
pub fn all_rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| parse_all().unwrap_or_else(|e| panic!("embedded rules are invalid: {e}")))
}

/// Registry keys a rule may touch (HKCU only, Explorer MRU lists).
pub const REGISTRY_ALLOWED_PREFIX: &str =
    "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\";

/// Validate a set of rules; returns human-readable problems (empty = valid).
pub fn validate(rules: &[Rule]) -> Vec<String> {
    let mut errs = Vec::new();
    let mut ids = HashSet::new();
    for r in rules {
        let id = &r.id;
        if !ids.insert(id.clone()) {
            errs.push(format!("duplicate rule id `{id}`"));
        }
        if id.is_empty()
            || !id.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
            })
            || !id.contains('.')
        {
            errs.push(format!("bad rule id `{id}` (want lowercase `group.name`)"));
        }
        for (what, v) in [
            ("name", &r.name),
            ("group", &r.group),
            ("description", &r.description),
        ] {
            if v.trim().is_empty() {
                errs.push(format!("{id}: empty {what}"));
            }
        }
        if r.os.is_empty() {
            errs.push(format!("{id}: no os listed"));
        }
        if r.targets.is_empty() {
            errs.push(format!("{id}: no targets"));
        }
        if let Some(w) = &r.warning {
            if w.trim().is_empty() {
                errs.push(format!("{id}: empty warning"));
            }
        }
        for name in [OsName::Linux, OsName::Windows, OsName::Macos] {
            let os = match name {
                OsName::Linux => Os::Linux,
                OsName::Windows => Os::Windows,
                OsName::Macos => Os::MacOs,
            };
            if r.os.contains(&name) && r.targets_for(os).next().is_none() {
                errs.push(format!(
                    "{id}: lists os `{}` but has no target for it",
                    name.as_str()
                ));
            }
        }
        for t in &r.targets {
            for o in t.os_list() {
                if !r.os.contains(o) {
                    errs.push(format!(
                        "{id}: target os `{}` is not in the rule's os list",
                        o.as_str()
                    ));
                }
            }
            validate_target(r, t, &mut errs);
        }
    }
    errs
}

fn check_template(rule: &Rule, target: &Target, tpl: &str, errs: &mut Vec<String>) {
    let id = &rule.id;
    match template::parse(tpl) {
        Err(e) => errs.push(format!("{id}: {e}")),
        Ok((var, comps)) => {
            // OS-only variables must only be used by targets that run on that OS only.
            let effective: Vec<OsName> = if target.os_list().is_empty() {
                rule.os.clone()
            } else {
                target.os_list().to_vec()
            };
            for (v, only) in template::OS_ONLY_VARS {
                if var == *v && effective.iter().any(|o| o.as_str() != *only) {
                    errs.push(format!(
                        "{id}: `{{{v}}}` is only valid for {only} targets: `{tpl}`"
                    ));
                }
            }
            for c in comps {
                if c.contains(['*', '?', '[', '{']) && globset::Glob::new(c).is_err() {
                    errs.push(format!("{id}: bad glob component `{c}` in `{tpl}`"));
                }
            }
        }
    }
}

fn check_glob(id: &str, what: &str, g: &str, errs: &mut Vec<String>) {
    if g.is_empty() || g.contains(['/', '\\']) {
        errs.push(format!("{id}: {what} `{g}` must be a non-empty name glob"));
    } else if globset::Glob::new(g).is_err() {
        errs.push(format!("{id}: invalid {what} glob `{g}`"));
    }
}

fn validate_target(rule: &Rule, t: &Target, errs: &mut Vec<String>) {
    let id = &rule.id;
    match t {
        Target::Files(f) => {
            check_template(rule, t, &f.base, errs);
            check_glob(id, "pattern", &f.pattern, errs);
            for s in &f.skip_names {
                check_glob(id, "skipNames entry", s, errs);
            }
            if f.min_age_hours.is_some() && f.min_age_from_settings {
                errs.push(format!("{id}: both minAgeHours and minAgeFromSettings"));
            }
            if f.literal_base.is_some() {
                errs.push(format!("{id}: literal_base must not come from a rule file"));
            }
        }
        Target::File(f) => check_template(rule, t, &f.path, errs),
        Target::Sqlite(s) => {
            check_template(rule, t, &s.db, errs);
            if s.statements.is_empty() || s.statements.iter().any(|x| x.trim().is_empty()) {
                errs.push(format!("{id}: sqlite target needs non-empty statements"));
            }
            if !s
                .count_query
                .trim()
                .to_ascii_uppercase()
                .starts_with("SELECT")
            {
                errs.push(format!("{id}: countQuery must be a SELECT"));
            }
            for st in &s.statements {
                let u = st.trim().to_ascii_uppercase();
                if !(u.starts_with("DELETE FROM ") || u.starts_with("UPDATE ")) {
                    errs.push(format!("{id}: statements must be DELETE/UPDATE: `{st}`"));
                }
            }
        }
        Target::Cookies(c) => check_template(rule, t, &c.db, errs),
        Target::Command(c) => {
            if c.program.is_empty()
                || !c
                    .program
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
            {
                errs.push(format!(
                    "{id}: command program `{}` must be a bare program name",
                    c.program
                ));
            }
        }
        Target::Trash(_) => {}
        Target::Registry(r) => {
            if !r.key.starts_with(REGISTRY_ALLOWED_PREFIX) {
                errs.push(format!(
                    "{id}: registry key must be under {REGISTRY_ALLOWED_PREFIX}: `{}`",
                    r.key
                ));
            }
            if !r.values && !r.subkeys {
                errs.push(format!(
                    "{id}: registry target must set values and/or subkeys"
                ));
            }
            let oses = if r.os.is_empty() {
                rule.os.clone()
            } else {
                r.os.clone()
            };
            if oses.iter().any(|o| *o != OsName::Windows) {
                errs.push(format!("{id}: registry targets are Windows only"));
            }
        }
    }
    if let Target::Sqlite(_) | Target::Cookies(_) = t {
        // fine
    }
}

/// Rules that apply to `os`, in file order.
pub fn rules_for_os(os: Os) -> Vec<&'static Rule> {
    all_rules().iter().filter(|r| r.applies_to(os)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_embedded_files_parse_and_validate() {
        let rules = parse_all().unwrap_or_else(|e| panic!("{e}"));
        assert!(rules.len() > 100, "only {} rules", rules.len());
        let errs = validate(&rules);
        assert!(errs.is_empty(), "rule problems:\n{}", errs.join("\n"));
    }

    #[test]
    fn ids_are_unique_and_every_os_has_rules_in_every_category() {
        let mut seen = HashSet::new();
        for r in all_rules() {
            assert!(seen.insert(&r.id), "duplicate {}", r.id);
        }
        for os in [Os::Linux, Os::Windows, Os::MacOs] {
            let rules = rules_for_os(os);
            for cat in [Category::Browser, Category::System, Category::Application] {
                assert!(
                    rules.iter().any(|r| r.category == cat),
                    "{os:?} has no {cat:?} rules"
                );
            }
        }
    }

    #[test]
    fn dangerous_defaults_are_off() {
        for r in all_rules() {
            let lower = r.id.to_lowercase();
            if lower.ends_with(".passwords") || lower.contains("password") {
                assert!(!r.default_enabled, "{} must not be on by default", r.id);
                assert!(r.warning.is_some(), "{} needs a warning", r.id);
            }
            if r.warning.is_some() && lower.contains("session") {
                assert!(!r.default_enabled, "{}", r.id);
            }
        }
    }

    #[test]
    fn processes_are_declared_for_browser_rules_on_each_os() {
        for r in all_rules()
            .iter()
            .filter(|r| r.category == Category::Browser)
        {
            for name in &r.os {
                let os = match name {
                    OsName::Linux => Os::Linux,
                    OsName::Windows => Os::Windows,
                    OsName::Macos => Os::MacOs,
                };
                assert!(
                    !r.processes.for_os(os).is_empty(),
                    "{} lists {} but names no process",
                    r.id,
                    name.as_str()
                );
                for p in r.processes.for_os(os) {
                    assert!(
                        !p.contains(['"', '\'', '\\', '\n']),
                        "{} process `{p}`",
                        r.id
                    );
                }
            }
        }
    }

    #[test]
    fn validator_catches_problems() {
        let bad = r#"
[[rule]]
id = "Bad Id"
name = "x"
group = "g"
category = "system"
os = ["linux", "windows"]
description = "d"
[[rule.targets]]
kind = "files"
os = ["linux"]
base = "/etc"
[[rule.targets]]
kind = "registry"
key = "HKLM\\Software\\Evil"
values = true
[[rule.targets]]
kind = "files"
base = "{appdata}/x"
"#;
        let f: RuleFile = toml::from_str(bad).unwrap();
        let errs = validate(&f.rule).join("\n");
        assert!(errs.contains("bad rule id"), "{errs}");
        assert!(errs.contains("must start with a {variable}"), "{errs}");
        assert!(errs.contains("registry key must be under"), "{errs}");
        assert!(errs.contains("only valid for windows"), "{errs}");
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let bad = r#"
[[rule]]
id = "a.b"
name = "x"
group = "g"
category = "system"
os = ["linux"]
description = "d"
surprise = 1
[[rule.targets]]
kind = "trash"
"#;
        assert!(toml::from_str::<RuleFile>(bad).is_err());
    }
}
