//! macOS startup items: launchd agents / daemons and login items.
//!
//! - Agents live in `~/Library/LaunchAgents` and `/Library/LaunchAgents`, daemons in
//!   `/Library/LaunchDaemons`. State comes from `launchctl print-disabled` (the override
//!   database) and falls back to the plist's own `Disabled` key.
//! - Disabling runs `launchctl disable <domain>/<label>` (daemons: elevated); the running job
//!   is only stopped (`launchctl bootout`) when the caller asks for it.
//! - Login items come from System Events and can only be deleted, not disabled.

use std::collections::HashMap;
use std::path::Path;

use crate::ctx::Ctx;
use crate::elevate::{applescript_escape, run_privileged};
use crate::error::{ApiError, Result};
use crate::pkgutil::summarize;

use super::impact::exe_from_command;
use super::model::{Entry, Kind, Scope, StartupItem, Target, ToggleOpts};

/// `launchctl print-disabled` -> `label -> disabled`. Understands both the
/// `=> disabled|enabled` and the older `=> true|false` spellings.
pub fn parse_print_disabled(out: &str) -> HashMap<String, bool> {
    let mut m = HashMap::new();
    for line in out.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix('"') else {
            continue;
        };
        let Some((label, tail)) = rest.split_once('"') else {
            continue;
        };
        let Some((_, value)) = tail.split_once("=>") else {
            continue;
        };
        match value.trim().trim_end_matches(';') {
            "disabled" | "true" => {
                m.insert(label.to_string(), true);
            }
            "enabled" | "false" => {
                m.insert(label.to_string(), false);
            }
            _ => {}
        }
    }
    m
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Agent {
    pub label: String,
    pub program: Option<String>,
    pub run_at_load: bool,
    pub disabled_key: bool,
}

pub fn parse_plist_file(path: &Path) -> Option<Agent> {
    let v = plist::Value::from_file(path).ok()?;
    let d = v.as_dictionary()?;
    let label = d.get("Label")?.as_string()?.to_string();
    let program = d
        .get("Program")
        .and_then(|p| p.as_string())
        .map(str::to_string)
        .or_else(|| {
            d.get("ProgramArguments")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_string())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .filter(|s| !s.is_empty())
        });
    Some(Agent {
        label,
        program,
        run_at_load: d
            .get("RunAtLoad")
            .and_then(|b| b.as_boolean())
            .unwrap_or(false),
        disabled_key: d
            .get("Disabled")
            .and_then(|b| b.as_boolean())
            .unwrap_or(false),
    })
}

fn label_ok(l: &str) -> bool {
    !l.is_empty()
        && !l.starts_with('-')
        && l.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
}

fn current_uid(ctx: &Ctx) -> Option<String> {
    let o = ctx.runner.run("id", &["-u"]).ok()?;
    let s = o.stdout.trim().to_string();
    (o.success() && !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then_some(s)
}

pub fn collect_launchd(ctx: &Ctx) -> Vec<Entry> {
    let uid = current_uid(ctx);
    let gui_domain = uid.as_ref().map(|u| format!("gui/{u}"));
    let user_over = gui_domain
        .as_ref()
        .and_then(|d| ctx.runner.run("launchctl", &["print-disabled", d]).ok())
        .map(|o| parse_print_disabled(&o.stdout))
        .unwrap_or_default();
    let system_over = ctx
        .runner
        .run("launchctl", &["print-disabled", "system"])
        .ok()
        .map(|o| parse_print_disabled(&o.stdout))
        .unwrap_or_default();

    let dirs = [
        (
            "home",
            ctx.env.home.join("Library/LaunchAgents"),
            false,
            true,
        ),
        (
            "agents",
            ctx.env.sys_path("/Library/LaunchAgents"),
            false,
            false,
        ),
        (
            "daemons",
            ctx.env.sys_path("/Library/LaunchDaemons"),
            true,
            false,
        ),
    ];
    let mut out = Vec::new();
    for (tag, dir, daemon, in_home) in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "plist") && p.is_file())
            .collect();
        files.sort();
        for path in files {
            let Some(a) = parse_plist_file(&path) else {
                continue;
            };
            let domain = if daemon {
                "system".to_string()
            } else {
                match &gui_domain {
                    Some(d) => d.clone(),
                    None => "gui/501".to_string(),
                }
            };
            let overrides = if daemon { &system_over } else { &user_over };
            let disabled = overrides.get(&a.label).copied().unwrap_or(a.disabled_key);
            let critical = a.label.to_lowercase().starts_with("com.apple.");
            let name = a
                .label
                .rsplit('.')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(&a.label)
                .to_string();
            let mut item = StartupItem::new(
                format!("mac:{tag}:{}", a.label),
                name,
                if daemon {
                    Kind::LaunchDaemon
                } else {
                    Kind::LaunchAgent
                },
                if in_home { Scope::User } else { Scope::System },
            );
            item.command = a.program.clone().unwrap_or_default();
            item.location = path.to_string_lossy().into_owned();
            item.enabled = !disabled;
            item.critical = critical;
            item.can_disable = !critical && label_ok(&a.label);
            item.can_delete = in_home && !critical;
            let mut e = Entry::new(
                item,
                Target::MacPlist {
                    path: path.clone(),
                    label: a.label.clone(),
                    domain,
                    daemon,
                    in_home,
                },
            );
            e.exe = a.program.as_deref().and_then(exe_from_command);
            out.push(e);
        }
    }
    out
}

pub fn set_enabled(ctx: &Ctx, target: &Target, enabled: bool, opts: ToggleOpts) -> Result<()> {
    let Target::MacPlist {
        label,
        domain,
        daemon,
        ..
    } = target
    else {
        return Err(ApiError::internal("not a launchd item"));
    };
    if !label_ok(label) {
        return Err(ApiError::invalid_params("unsupported launchd label"));
    }
    let svc = format!("{domain}/{label}");
    let verb = if enabled { "enable" } else { "disable" };
    let out = if *daemon {
        run_privileged(ctx, "launchctl", &[verb, &svc])?
    } else {
        ctx.runner.run("launchctl", &[verb, &svc])?
    };
    if !out.success() {
        return Err(ApiError::io(format!(
            "launchctl {verb} failed: {}",
            summarize(&out)
        )));
    }
    if !enabled && opts.stop_now {
        // Not loaded is fine.
        let _ = if *daemon {
            run_privileged(ctx, "launchctl", &["bootout", &svc])
        } else {
            ctx.runner.run("launchctl", &["bootout", &svc])
        };
    }
    Ok(())
}

// ---------------------------------------------------------------- login items

pub const LOGIN_ITEMS_SCRIPT: &str = "tell application \"System Events\"\n\
     set out to \"\"\n\
     repeat with li in every login item\n\
     set out to out & (name of li) & tab & (path of li) & linefeed\n\
     end repeat\n\
     return out\n\
     end tell";

/// `name<TAB>path` lines.
pub fn parse_login_items(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| {
            let (n, p) = l.split_once('\t')?;
            let n = n.trim();
            (!n.is_empty()).then(|| (n.to_string(), p.trim().to_string()))
        })
        .collect()
}

pub fn collect_login_items(ctx: &Ctx) -> Vec<Entry> {
    let Ok(o) = ctx.runner.run("osascript", &["-e", LOGIN_ITEMS_SCRIPT]) else {
        return Vec::new();
    };
    if !o.success() {
        return Vec::new();
    }
    parse_login_items(&o.stdout)
        .into_iter()
        .map(|(name, path)| {
            let mut item = StartupItem::new(
                format!("mac:login:{name}"),
                name.clone(),
                Kind::LoginItem,
                Scope::User,
            );
            item.command = path.clone();
            item.location = "Login Items".into();
            item.can_disable = false;
            item.can_delete = true;
            let mut e = Entry::new(item, Target::MacLogin { name, path: path.clone() });
            e.exe = Some(path);
            e
        })
        .collect()
}

pub fn delete_login_item_script(name: &str) -> String {
    format!(
        "tell application \"System Events\" to delete login item \"{}\"",
        applescript_escape(name)
    )
}

pub fn make_login_item_script(path: &str) -> String {
    format!(
        "tell application \"System Events\" to make login item at end with properties {{path:\"{}\", hidden:false}}",
        applescript_escape(path)
    )
}

pub fn delete_login_item(ctx: &Ctx, name: &str) -> Result<()> {
    let out = ctx
        .runner
        .run("osascript", &["-e", &delete_login_item_script(name)])?;
    if out.success() {
        Ok(())
    } else {
        Err(ApiError::io(format!(
            "could not delete the login item: {}",
            summarize(&out)
        )))
    }
}
