//! System restore: list, create and delete restore points, and put ClearSweep's own backups back.
//!
//! Methods:
//! - `restore.list_points { elevate? }`: `{ supported, tool, tools, hint, canCreate, canDeleteOld,
//!   canOpenSystemTool, needsAdmin, warnings, points }`. Points come from the operating system
//!   (Windows restore points, Timeshift, Snapper, Time Machine local snapshots) and from
//!   ClearSweep's own backups (`<data>/backups`, kind `clearsweep-backup`).
//! - `restore.create_point { description }` (administrator rights).
//! - `restore.delete_point { id }`: never the most recent operating-system point; Windows cannot
//!   delete single points (see `restore.delete_old`).
//! - `restore.delete_old`: Windows, deletes every restore point except the most recent one.
//! - `restore.restore { id }`: ClearSweep backups only. An operating-system rollback must be done
//!   with the system's own tool (`restore.open_system_tool` opens it where there is one).
//!
//! Ids sent by the client are validated against a strict grammar and, for anything that changes
//! the system, looked up again in a fresh listing.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::api::Registry;
use crate::ctx::{Ctx, Os};
use crate::elevate::{powershell_encode, run_privileged};
use crate::error::{ApiError, Result};
use crate::features::registry_cleaner::backup::{self, classify_name, BackupKind};
use crate::features::registry_cleaner::regcmd::{run_direct, run_privileged_batch, RegCmd};
use crate::job::Job;
use crate::pkgutil::{path_size, summarize};
use crate::runner::CmdOutput;

pub mod parse;
#[cfg(test)]
mod tests;

use parse::RawPoint;

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "restore.list_points",
    "restore.create_point",
    "restore.delete_point",
    "restore.delete_old",
    "restore.restore",
    "restore.open_system_tool",
];

pub fn register(r: &mut Registry) {
    r.add("restore.list_points", list_handler);
    r.add("restore.create_point", create_handler);
    r.add("restore.delete_point", delete_handler);
    r.add("restore.delete_old", delete_old_handler);
    r.add("restore.restore", restore_handler);
    r.add("restore.open_system_tool", open_tool_handler);
}

// ---------------------------------------------------------------- model

pub const KIND_WINDOWS: &str = "windows-restore-point";
pub const KIND_TIMESHIFT: &str = "timeshift";
pub const KIND_SNAPPER: &str = "snapper";
pub const KIND_TMUTIL: &str = "tmutil";
pub const KIND_BACKUP: &str = "clearsweep-backup";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    pub id: String,
    pub description: String,
    pub created_at: Option<String>,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    pub deletable: bool,
    /// ClearSweep can put this back itself.
    pub restorable: bool,
    /// The newest point of its tool: never deletable.
    pub is_newest: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// For ClearSweep backups: `registry`, `config`, `uninstall-entry` or `drivers`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_kind: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Windows,
    Timeshift,
    Snapper,
    Tmutil,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Tool::Windows => "windows",
            Tool::Timeshift => "timeshift",
            Tool::Snapper => "snapper",
            Tool::Tmutil => "tmutil",
        }
    }
    fn kind(self) -> &'static str {
        match self {
            Tool::Windows => KIND_WINDOWS,
            Tool::Timeshift => KIND_TIMESHIFT,
            Tool::Snapper => KIND_SNAPPER,
            Tool::Tmutil => KIND_TMUTIL,
        }
    }
}

fn detect(ctx: &Ctx) -> Vec<Tool> {
    let has = |p: &str| ctx.runner.which(p).is_some();
    match ctx.env.os {
        Os::Windows => has("powershell")
            .then_some(Tool::Windows)
            .into_iter()
            .collect(),
        Os::Linux => [(Tool::Timeshift, "timeshift"), (Tool::Snapper, "snapper")]
            .into_iter()
            .filter(|(_, p)| has(p))
            .map(|(t, _)| t)
            .collect(),
        Os::MacOs => has("tmutil").then_some(Tool::Tmutil).into_iter().collect(),
    }
}

fn hint_for(os: Os) -> &'static str {
    match os {
        Os::Windows => "Windows PowerShell was not found, so restore points cannot be managed from here.",
        Os::Linux => "No snapshot tool found. Install Timeshift (for example `sudo apt install timeshift`) or Snapper to create and manage system restore points.",
        Os::MacOs => "Time Machine's `tmutil` was not found.",
    }
}

// ---------------------------------------------------------------- ids

fn is_snapper_config(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with(['-', '.'])
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
}

fn all_digits(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit())
}

fn is_timeshift_name(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 19
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 | 13 | 16 => *c == b'-',
            10 => *c == b'_',
            _ => c.is_ascii_digit(),
        })
}

fn is_tmutil_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 17
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 | 10 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// A parsed, validated point id.
#[derive(Debug, Clone, PartialEq)]
enum PointId {
    Windows(String),
    Timeshift(String),
    Snapper(String, String),
    Tmutil(String),
    Backup(String),
}

fn parse_id(id: &str) -> Result<PointId> {
    let bad = || ApiError::invalid_params(format!("`{id}` is not a restore point id"));
    let (tool, rest) = id.split_once(':').ok_or_else(bad)?;
    match tool {
        "windows" if all_digits(rest, 10) => Ok(PointId::Windows(rest.into())),
        "timeshift" if is_timeshift_name(rest) => Ok(PointId::Timeshift(rest.into())),
        "snapper" => {
            let (cfg, n) = rest.split_once(':').ok_or_else(bad)?;
            if is_snapper_config(cfg) && all_digits(n, 9) && n != "0" {
                Ok(PointId::Snapper(cfg.into(), n.into()))
            } else {
                Err(bad())
            }
        }
        "tmutil" if is_tmutil_date(rest) => Ok(PointId::Tmutil(rest.into())),
        "clearsweep" if classify_name(rest).is_some() => Ok(PointId::Backup(rest.into())),
        _ => Err(bad()),
    }
}

// ---------------------------------------------------------------- listing

struct Listed {
    point: Point,
    group: String,
    order: i128,
}

struct OsListing {
    points: Vec<Point>,
    needs_admin: bool,
    warnings: Vec<String>,
}

fn run_tool(ctx: &Ctx, elevate: bool, prog: &str, args: &[&str]) -> Result<CmdOutput> {
    if elevate {
        run_privileged(ctx, prog, args)
    } else {
        ctx.runner.run(prog, args)
    }
}

fn looks_denied(o: &CmdOutput) -> bool {
    let t = format!("{}\n{}", o.stdout, o.stderr).to_lowercase();
    [
        "root",
        "permission",
        "access is denied",
        "not permitted",
        "privilege",
        "administrator",
    ]
    .iter()
    .any(|w| t.contains(w))
}

const WIN_LIST_SCRIPT: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-ComputerRestorePoint | Select SequenceNumber,Description,CreationTime,RestorePointType | ConvertTo-Json -Compress";

fn to_listed(tool: Tool, group: &str, id: String, raw: RawPoint, deletable: bool) -> Listed {
    Listed {
        group: group.to_string(),
        order: raw.order,
        point: Point {
            id,
            description: raw.description,
            created_at: raw.created_at,
            kind: tool.kind(),
            size_bytes: None,
            deletable,
            restorable: false,
            is_newest: false,
            note: raw.note,
            backup_kind: None,
        },
    }
}

fn list_os(ctx: &Ctx, tools: &[Tool], elevate: bool) -> OsListing {
    let mut listed: Vec<Listed> = Vec::new();
    let mut needs_admin = false;
    let mut warnings = Vec::new();
    for &tool in tools {
        match tool {
            Tool::Windows => {
                match run_tool(
                    ctx,
                    elevate,
                    "powershell",
                    &["-NoProfile", "-NonInteractive", "-Command", WIN_LIST_SCRIPT],
                ) {
                    Ok(o) => {
                        let pts = parse::parse_windows_points(&o.stdout);
                        if pts.is_empty() && (!o.success() || looks_denied(&o)) && !elevate {
                            needs_admin = true;
                        } else if pts.is_empty() && !o.success() {
                            warnings
                                .push(format!("Could not read restore points: {}", summarize(&o)));
                        }
                        for p in pts {
                            let id = format!("windows:{}", p.key);
                            listed.push(to_listed(tool, "windows", id, p, false));
                        }
                    }
                    Err(e) => warnings.push(e.message),
                }
            }
            Tool::Timeshift => match run_tool(ctx, elevate, "timeshift", &["--list"]) {
                Ok(o) => {
                    let pts = parse::parse_timeshift_list(&o.stdout);
                    if pts.is_empty() && (!o.success() || looks_denied(&o)) && !elevate {
                        needs_admin = true;
                    }
                    for p in pts {
                        let id = format!("timeshift:{}", p.key);
                        listed.push(to_listed(tool, "timeshift", id, p, true));
                    }
                }
                Err(e) => warnings.push(e.message),
            },
            Tool::Snapper => {
                let configs = match run_tool(ctx, elevate, "snapper", &["list-configs"]) {
                    Ok(o) => {
                        let c = parse::parse_snapper_configs(&o.stdout);
                        if c.is_empty() && (!o.success() || looks_denied(&o)) && !elevate {
                            needs_admin = true;
                        }
                        c
                    }
                    Err(e) => {
                        warnings.push(e.message);
                        Vec::new()
                    }
                };
                for cfg in configs.iter().filter(|c| is_snapper_config(c)) {
                    match run_tool(ctx, elevate, "snapper", &["--iso", "-c", cfg, "list"]) {
                        Ok(o) => {
                            for p in parse::parse_snapper_list(cfg, &o.stdout) {
                                let id = format!("snapper:{}", p.key);
                                listed.push(to_listed(
                                    tool,
                                    &format!("snapper:{cfg}"),
                                    id,
                                    p,
                                    true,
                                ));
                            }
                        }
                        Err(e) => warnings.push(e.message),
                    }
                }
            }
            Tool::Tmutil => match ctx.runner.run("tmutil", &["listlocalsnapshots", "/"]) {
                Ok(o) => {
                    for p in parse::parse_tmutil(&o.stdout) {
                        let id = format!("tmutil:{}", p.key);
                        listed.push(to_listed(tool, "tmutil", id, p, true));
                    }
                }
                Err(e) => warnings.push(e.message),
            },
        }
    }
    // The newest point of each group is protected.
    let groups: std::collections::HashSet<String> =
        listed.iter().map(|l| l.group.clone()).collect();
    for g in groups {
        let max = listed
            .iter()
            .filter(|l| l.group == g)
            .map(|l| l.order)
            .max();
        for l in listed
            .iter_mut()
            .filter(|l| l.group == g && Some(l.order) == max)
        {
            l.point.is_newest = true;
            l.point.deletable = false;
            if l.point.kind != KIND_WINDOWS {
                l.point.note = Some(match l.point.note.take() {
                    Some(n) => format!("{n}; most recent"),
                    None => "most recent".to_string(),
                });
            }
        }
    }
    listed.sort_by(|a, b| a.group.cmp(&b.group).then(b.order.cmp(&a.order)));
    OsListing {
        points: listed.into_iter().map(|l| l.point).collect(),
        needs_admin,
        warnings,
    }
}

fn iso_from_unix(ts: i64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(ts)
        .ok()?
        .format(&Rfc3339)
        .ok()
}

fn ts_of(name: &str) -> Option<i64> {
    let digits: String = name
        .split_once('-')?
        .1
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// ClearSweep's own backups.
fn list_backups(ctx: &Ctx) -> Vec<Point> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(backup::backups_dir(ctx)) else {
        return out;
    };
    let windows = ctx.env.os == Os::Windows;
    let platform = backup::platform_name(ctx.env.os);
    for e in rd.filter_map(|e| e.ok()) {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(kind) = classify_name(&name) else {
            continue;
        };
        let path = e.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let (description, created, restorable, bk) = match kind {
            BackupKind::Registry | BackupKind::Config => {
                let Ok(m) = backup::read_manifest(&path) else {
                    continue; // aborted attempt without a manifest
                };
                let what = if kind == BackupKind::Registry {
                    "Registry backup"
                } else {
                    "Configuration backup"
                };
                (
                    format!(
                        "{what} ({} {})",
                        m.issues.len(),
                        if m.issues.len() == 1 { "item" } else { "items" }
                    ),
                    Some(m.created_at.clone()),
                    m.platform == platform,
                    if kind == BackupKind::Registry {
                        "registry"
                    } else {
                        "config"
                    },
                )
            }
            BackupKind::UninstallEntry => (
                "Uninstall list entry backup".to_string(),
                ts_of(&name).and_then(iso_from_unix),
                windows,
                "uninstall-entry",
            ),
            BackupKind::Drivers => (
                "Driver backup".to_string(),
                ts_of(&name).and_then(iso_from_unix),
                windows,
                "drivers",
            ),
        };
        out.push(Point {
            id: format!("clearsweep:{name}"),
            description,
            created_at: created,
            kind: KIND_BACKUP,
            size_bytes: Some(if meta.is_dir() {
                path_size(&path)
            } else {
                meta.len()
            }),
            deletable: true,
            restorable,
            is_newest: false,
            note: None,
            backup_kind: Some(bk),
        });
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
    out
}

fn can_open_tool(ctx: &Ctx, tools: &[Tool]) -> bool {
    match ctx.env.os {
        Os::Windows => tools.contains(&Tool::Windows),
        Os::MacOs => tools.contains(&Tool::Tmutil),
        Os::Linux => ctx.runner.which("timeshift-launcher").is_some(),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ListParams {
    elevate: bool,
}

fn list_handler(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: ListParams = if params.is_null() {
        ListParams::default()
    } else {
        serde_json::from_value(params)?
    };
    let tools = detect(ctx);
    let os = list_os(ctx, &tools, p.elevate);
    let mut points = os.points;
    points.extend(list_backups(ctx));
    Ok(json!({
        "os": backup::platform_name(ctx.env.os),
        "supported": !tools.is_empty(),
        "tool": tools.first().map(|t| t.name()),
        "tools": tools.iter().map(|t| t.name()).collect::<Vec<_>>(),
        "hint": tools.is_empty().then(|| hint_for(ctx.env.os)),
        "canCreate": !tools.is_empty(),
        "canDeleteOld": tools.contains(&Tool::Windows),
        "canOpenSystemTool": can_open_tool(ctx, &tools),
        "needsAdmin": os.needs_admin,
        "warnings": os.warnings,
        "points": points,
    }))
}

// ---------------------------------------------------------------- create

#[derive(Deserialize)]
struct CreateParams {
    description: String,
}

fn valid_description(d: &str) -> Result<String> {
    let d = d.trim();
    if d.is_empty() || d.chars().count() > 200 {
        return Err(ApiError::invalid_params(
            "The description must be 1 to 200 characters",
        ));
    }
    if d.chars().any(char::is_control) {
        return Err(ApiError::invalid_params(
            "The description must not contain control characters",
        ));
    }
    if d.starts_with('-') {
        return Err(ApiError::invalid_params(
            "The description must not start with a dash",
        ));
    }
    Ok(d.to_string())
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The PowerShell that creates a restore point and reports what happened on stdout.
pub fn windows_create_script(description: &str) -> String {
    format!(
        "[Console]::OutputEncoding=[Text.Encoding]::UTF8; $ErrorActionPreference='Stop'; \
         try {{ Checkpoint-Computer -Description {} -RestorePointType MODIFY_SETTINGS -WarningVariable w -WarningAction SilentlyContinue; \
         if ($w) {{ 'WARN: ' + ($w -join ' ') }} else {{ 'CREATED' }} }} \
         catch {{ 'ERROR: ' + $_.Exception.Message; exit 1 }}",
        ps_quote(description)
    )
}

fn create_handler(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: CreateParams = serde_json::from_value(params)?;
    let desc = valid_description(&p.description)?;
    let tools = detect(ctx);
    let Some(tool) = tools.first().copied() else {
        return Err(ApiError::unsupported(hint_for(ctx.env.os)));
    };
    let out = match tool {
        Tool::Windows => {
            let enc = powershell_encode(&windows_create_script(&desc));
            let o = run_privileged(
                ctx,
                "powershell",
                &[
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-EncodedCommand",
                    &enc,
                ],
            )?;
            let text = format!("{}\n{}", o.stdout, o.stderr);
            let line = text
                .lines()
                .map(str::trim)
                .find(|l| {
                    l.starts_with("CREATED") || l.starts_with("WARN:") || l.starts_with("ERROR:")
                })
                .unwrap_or("")
                .to_string();
            if line == "CREATED" && o.success() {
                return Ok(
                    json!({ "ok": true, "throttled": false, "message": "Restore point created." }),
                );
            }
            let lower = line.to_lowercase();
            let throttled = lower.contains("1440")
                || lower.contains("already been created")
                || lower.contains("within the past");
            let message = if throttled {
                "Windows allows one automatic restore point every 24 hours, and one was created recently. Try again later.".to_string()
            } else if line.is_empty() {
                format!("Creating the restore point failed: {}", summarize(&o))
            } else {
                format!(
                    "Creating the restore point failed: {}",
                    line.trim_start_matches("WARN: ")
                        .trim_start_matches("ERROR: ")
                )
            };
            return Ok(json!({ "ok": false, "throttled": throttled, "message": message }));
        }
        Tool::Timeshift => run_privileged(
            ctx,
            "timeshift",
            &["--create", "--comments", &desc, "--tags", "O", "--scripted"],
        )?,
        Tool::Snapper => run_privileged(
            ctx,
            "snapper",
            &[
                "-c",
                "root",
                "create",
                "--type",
                "single",
                "--description",
                &desc,
            ],
        )?,
        Tool::Tmutil => run_privileged(ctx, "tmutil", &["localsnapshot"])?,
    };
    if out.success() {
        Ok(json!({ "ok": true, "throttled": false, "message": "Restore point created." }))
    } else {
        Ok(
            json!({ "ok": false, "throttled": false, "message": format!("Creating the restore point failed: {}", summarize(&out)) }),
        )
    }
}

// ---------------------------------------------------------------- delete

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

fn delete_handler(ctx: &Ctx, params: Value, _job: &Job) -> Result<Value> {
    let p: IdParams = serde_json::from_value(params)?;
    let id = parse_id(&p.id)?;
    match id {
        PointId::Backup(name) => {
            let freed = backup::delete_backup_entry(ctx, &name)?;
            Ok(json!({ "ok": true, "id": p.id, "freedBytes": freed, "message": "Backup deleted." }))
        }
        PointId::Windows(_) => Err(ApiError::unsupported(
            "Windows can only delete restore points in bulk. Use \"Delete all but the most recent\" instead.",
        )),
        other => {
            let tools = detect(ctx);
            // Look the point up again, and never remove the newest one.
            let listing = list_os(ctx, &tools, true);
            let Some(pt) = listing.points.iter().find(|x| x.id == p.id) else {
                return Err(ApiError::not_found("That restore point no longer exists"));
            };
            if pt.is_newest || !pt.deletable {
                return Err(ApiError::invalid_params(
                    "The most recent restore point cannot be deleted",
                ));
            }
            let out = match &other {
                PointId::Timeshift(name) => run_privileged(ctx, "timeshift", &["--delete", "--snapshot", name, "--yes", "--scripted"])?,
                PointId::Snapper(cfg, n) => run_privileged(ctx, "snapper", &["-c", cfg, "delete", n])?,
                PointId::Tmutil(date) => run_privileged(ctx, "tmutil", &["deletelocalsnapshots", date])?,
                _ => unreachable!(),
            };
            let ok = out.success();
            Ok(json!({
                "ok": ok,
                "id": p.id,
                "message": if ok { "Restore point deleted.".to_string() } else { format!("Deleting failed: {}", summarize(&out)) },
            }))
        }
    }
}

fn system_drive(ctx: &Ctx) -> String {
    let r = ctx.env.root.to_string_lossy();
    let b = r.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        format!("{}:", (b[0] as char).to_ascii_uppercase())
    } else {
        "C:".to_string()
    }
}

fn delete_old_handler(ctx: &Ctx, _params: Value, job: &Job) -> Result<Value> {
    if ctx.env.os != Os::Windows {
        return Err(ApiError::unsupported(
            "Deleting older restore points in bulk is only available on Windows",
        ));
    }
    let tools = detect(ctx);
    if !tools.contains(&Tool::Windows) {
        return Err(ApiError::unsupported(hint_for(Os::Windows)));
    }
    job.check_cancelled()?;
    let before = list_os(ctx, &tools, true);
    let n = before.points.len();
    if n <= 1 {
        return Ok(
            json!({ "ok": true, "deleted": 0, "remaining": n, "message": "Only the most recent restore point exists; nothing to delete." }),
        );
    }
    // At most n-1 deletions of the oldest shadow copy: the newest restore point cannot be reached.
    let script = format!(
        "$d=0; for($i=0;$i -lt {};$i++){{ vssadmin delete shadows /for={} /oldest /quiet | Out-Null; if($LASTEXITCODE -ne 0){{ 'FAIL ' + $LASTEXITCODE; break }}; $d++; 'OK' }}",
        n - 1,
        system_drive(ctx)
    );
    let enc = powershell_encode(&script);
    let o = run_privileged(
        ctx,
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &enc,
        ],
    )?;
    let deleted = o.stdout.lines().filter(|l| l.trim() == "OK").count();
    let failed = o.stdout.lines().any(|l| l.trim().starts_with("FAIL"));
    let after = list_os(ctx, &tools, true).points.len();
    let ok = !failed && o.success() && deleted == n - 1;
    Ok(json!({
        "ok": ok,
        "deleted": deleted,
        "remaining": after,
        "message": if ok {
            format!("Deleted {deleted} older restore point{}; the most recent one was kept.", if deleted == 1 { "" } else { "s" })
        } else {
            format!("Deleted {deleted} of {} older restore points. {}", n - 1, summarize(&o))
        },
    }))
}

// ---------------------------------------------------------------- restore

fn reg_file_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let u: Vec<u16> = bytes[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn reg_file_needs_admin(text: &str) -> bool {
    let up = text.to_uppercase();
    [
        "[HKEY_LOCAL_MACHINE",
        "[-HKEY_LOCAL_MACHINE",
        "[HKEY_CLASSES_ROOT",
        "[-HKEY_CLASSES_ROOT",
    ]
    .iter()
    .any(|k| up.contains(k))
}

fn restore_handler(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: IdParams = serde_json::from_value(params)?;
    let name = match parse_id(&p.id)? {
        PointId::Backup(n) => n,
        PointId::Windows(_) => {
            return Err(ApiError::unsupported(
                "Roll back with Windows System Restore (rstrui.exe): use \"Open System Restore\". ClearSweep does not roll the system back itself.",
            ))
        }
        PointId::Timeshift(_) => {
            return Err(ApiError::unsupported(
                "Roll back from Timeshift itself (open Timeshift and choose Restore, or boot from a live system). ClearSweep does not roll the system back itself.",
            ))
        }
        PointId::Snapper(..) => {
            return Err(ApiError::unsupported(
                "Roll back with `snapper rollback` or your bootloader's snapshot menu. ClearSweep does not roll the system back itself.",
            ))
        }
        PointId::Tmutil(_) => {
            return Err(ApiError::unsupported(
                "Roll back through Time Machine (Enter Time Machine) or macOS Recovery. ClearSweep does not roll the system back itself.",
            ))
        }
    };
    let path = backup::backups_dir(ctx).join(&name);
    if fs::symlink_metadata(&path).is_err() {
        return Err(ApiError::not_found(format!(
            "backup `{name}` does not exist"
        )));
    }
    match classify_name(&name) {
        Some(BackupKind::Registry | BackupKind::Config) => {
            let out = backup::restore(ctx, &name, job)?;
            let mut v = serde_json::to_value(&out)?;
            v["message"] = json!(if out.ok {
                format!(
                    "Restored {} item{}.",
                    out.restored,
                    if out.restored == 1 { "" } else { "s" }
                )
            } else {
                format!(
                    "{} restored, {} could not be restored.",
                    out.restored, out.failed
                )
            });
            Ok(v)
        }
        Some(BackupKind::UninstallEntry) => {
            if ctx.env.os != Os::Windows {
                return Err(ApiError::unsupported(
                    "This backup can only be restored on Windows",
                ));
            }
            let bytes = fs::read(&path)?;
            let cmd = RegCmd::import(&path.to_string_lossy());
            let r = if reg_file_needs_admin(&reg_file_text(&bytes)) {
                run_privileged_batch(ctx, std::slice::from_ref(&cmd)).remove(0)
            } else {
                run_direct(ctx, &cmd)
            };
            Ok(
                json!({ "ok": r.ok, "message": if r.ok { "Registry entry restored.".to_string() } else { r.message } }),
            )
        }
        Some(BackupKind::Drivers) => {
            if ctx.env.os != Os::Windows {
                return Err(ApiError::unsupported(
                    "Driver backups can only be restored on Windows",
                ));
            }
            let pattern = format!("{}\\*.inf", path.to_string_lossy().trim_end_matches('\\'));
            let o = run_privileged(
                ctx,
                "pnputil",
                &["/add-driver", &pattern, "/subdirs", "/install"],
            )?;
            let ok = o.success() || o.status == 3010;
            Ok(
                json!({ "ok": ok, "message": if ok { "Drivers reinstalled from the backup.".to_string() } else { format!("Reinstalling drivers failed: {}", summarize(&o)) } }),
            )
        }
        None => Err(ApiError::invalid_params("not a ClearSweep backup")),
    }
}

// ---------------------------------------------------------------- system tool

fn open_tool_handler(ctx: &Ctx, _params: Value, _job: &Job) -> Result<Value> {
    let tools = detect(ctx);
    let r = match ctx.env.os {
        Os::Windows if tools.contains(&Tool::Windows) => {
            ctx.runner.run("cmd", &["/c", "start", "", "rstrui.exe"])?
        }
        Os::MacOs if tools.contains(&Tool::Tmutil) => {
            ctx.runner.run("open", &["-a", "Time Machine"])?
        }
        Os::Linux if ctx.runner.which("timeshift-launcher").is_some() => {
            // detached: the GUI must not block this call
            ctx.runner
                .run("sh", &["-c", "nohup timeshift-launcher >/dev/null 2>&1 &"])?
        }
        _ => {
            return Err(ApiError::unsupported(
                "There is no graphical restore tool to open on this system",
            ))
        }
    };
    let ok = r.success();
    Ok(
        json!({ "ok": ok, "message": if ok { "Opened.".to_string() } else { format!("Could not open it: {}", summarize(&r)) } }),
    )
}
