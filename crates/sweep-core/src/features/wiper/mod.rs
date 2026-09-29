//! Drive wiper: overwrite the free space of a volume, or a whole drive.
//!
//! Methods (JSON, camelCase):
//! - `wiper.list_drives` -> `[{name, mount, fs, total, available, removable, isSystem,
//!   device?, wholeDisk?}]` (mounted volumes)
//! - `wiper.list_devices` -> `{supported, devices: [{device, sizeBytes, removable, model,
//!   partitions, mounts, swap, holders, isSystem}]}` (whole disks; Linux)
//! - `wiper.wipe_free_space { mount, passes: 1|3|7|35 }` -> report (see [`freespace`])
//! - `wiper.wipe_drive { device, passes, confirm }` -> report (see [`drive`])
//!
//! Overwriting cannot be guaranteed on SSDs (wear levelling, TRIM), copy-on-write or
//! journaling filesystems and snapshotted volumes; the UI says so.

pub mod devices;
pub mod drive;
pub mod freespace;
#[cfg(test)]
pub(crate) mod tests;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::api::Registry;
use crate::ctx::{Ctx, Os};
use crate::error::{ApiError, Result};
use crate::features::disk_analyzer::drives::{list_volumes, Volume};
use crate::features::secure_delete::overwrite_passes;
use crate::job::Job;
use crate::safety::{is_within, key_of_path, ExcludeSet, Safety};

use drive::{valid_passes, DriveBackend, RealBackend};
use freespace::{fill_free_space, FillParams, FillReport, RealFs, WipeFs, MAX_FILE_BYTES};

/// Method names owned by this feature.
pub const METHODS: &[&str] = &[
    "wiper.list_drives",
    "wiper.list_devices",
    "wiper.wipe_free_space",
    "wiper.wipe_drive",
];

pub fn register(r: &mut Registry) {
    r.add("wiper.list_drives", list_drives);
    r.add("wiper.list_devices", list_devices);
    r.add("wiper.wipe_free_space", wipe_free_space);
    r.add("wiper.wipe_drive", wipe_drive);
}

// ---------------------------------------------------------------- list_drives

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DriveInfo {
    #[serde(flatten)]
    volume: Volume,
    /// Contains the operating system: entire-drive wiping is never offered for it.
    is_system: bool,
    /// Device node / drive letter of the volume (`/dev/sda1`, `C:`, `/dev/disk3s1`).
    #[serde(skip_serializing_if = "Option::is_none")]
    device: Option<String>,
    /// The physical disk that volume lives on (`/dev/sda`), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    whole_disk: Option<String>,
}

/// Is a volume mounted at `mount` part of the operating system?
pub fn is_system_mount(ctx: &Ctx, mount: &str, root_volume: Option<&str>) -> bool {
    if root_volume == Some(mount) {
        return true;
    }
    match ctx.env.os {
        Os::Linux => drive::linux_system_mount(mount),
        Os::MacOs => mount == "/" || mount == "/System/Volumes/Data",
        Os::Windows => {
            let letter = |s: &str| s.chars().take(2).collect::<String>().to_ascii_uppercase();
            let root = ctx.env.root.to_string_lossy();
            letter(mount) == letter(&root)
        }
    }
}

/// The mount that contains the environment's root directory (the system drive).
fn root_volume<'a>(ctx: &Ctx, vols: &'a [Volume]) -> Option<&'a str> {
    vols.iter()
        .filter(|v| is_within(&ctx.env.root, Path::new(&v.mount)))
        .max_by_key(|v| Path::new(&v.mount).components().count())
        .map(|v| v.mount.as_str())
}

fn macos_device(ctx: &Ctx, mount: &str) -> (Option<String>, Option<String>) {
    let Ok(out) = ctx.runner.run("diskutil", &["info", mount]) else {
        return (None, None);
    };
    if !out.success() {
        return (None, None);
    }
    let field = |label: &str| {
        out.stdout.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == label).then(|| v.trim().to_string())
        })
    };
    let whole = field("Part of Whole")
        .as_deref()
        .and_then(drive::disk_base)
        .map(|b| format!("/dev/{b}"));
    (field("Device Node"), whole)
}

fn list_drives(ctx: &Ctx, _params: Value, _job: &Job) -> Result<Value> {
    let vols = list_volumes(ctx);
    let root = root_volume(ctx, &vols).map(str::to_string);
    let block = devices::list_block_devices(&ctx.env);
    let mounts = devices::read_mounts(&ctx.env);
    let infos: Vec<DriveInfo> = vols
        .into_iter()
        .map(|v| {
            let is_system = is_system_mount(ctx, &v.mount, root.as_deref());
            let (device, whole_disk) = match ctx.env.os {
                Os::Linux => {
                    let source = mounts
                        .iter()
                        .find(|m| m.mount == v.mount)
                        .map(|m| m.source.clone())
                        .or_else(|| v.name.starts_with("/dev/").then(|| v.name.clone()))
                        .filter(|s| s.starts_with("/dev/"));
                    let whole = source
                        .as_deref()
                        .and_then(|s| devices::whole_disk_of(&block, s));
                    (source, whole)
                }
                Os::Windows => (
                    Some(v.mount.trim_end_matches(['\\', '/']).to_string()),
                    None,
                ),
                Os::MacOs => macos_device(ctx, &v.mount),
            };
            DriveInfo {
                volume: v,
                is_system,
                device,
                whole_disk,
            }
        })
        .collect();
    Ok(serde_json::to_value(infos)?)
}

// ---------------------------------------------------------------- list_devices

fn list_devices(ctx: &Ctx, _params: Value, _job: &Job) -> Result<Value> {
    if ctx.env.os != Os::Linux {
        return Ok(json!({"supported": false, "devices": []}));
    }
    let devs: Vec<Value> = RealBackend
        .devices(ctx)
        .into_iter()
        .map(|d| {
            json!({
                "device": d.device.device,
                "name": d.device.name,
                "sizeBytes": d.device.size_bytes,
                "removable": d.device.removable,
                "model": d.device.model,
                "partitions": d.device.partitions,
                "mounts": d.usage.mounts,
                "swap": d.usage.swap,
                "holders": d.usage.holders,
                "isSystem": d.is_system,
                "mountsKnown": d.mounts_known,
            })
        })
        .collect();
    Ok(json!({"supported": true, "devices": devs}))
}

// ---------------------------------------------------------------- wipe_free_space

#[derive(Debug, Deserialize)]
struct FreeParams {
    mount: String,
    passes: u32,
}

fn report_json(mount: &Path, passes: u32, r: &FillReport) -> Value {
    json!({
        "mount": mount.to_string_lossy(),
        "passes": passes,
        "filesWritten": r.files,
        "bytesPerPass": r.bytes_per_pass,
        "bytesWritten": r.bytes_written,
        "freeBefore": r.free_before,
        "freeAfter": r.free_after,
        "location": r.location.to_string_lossy(),
        "staleRemoved": r.stale_removed,
        "durationMs": r.duration_ms,
    })
}

/// Wipe the free space of `mount` (which the caller has validated) using `fs`.
pub fn run_free_space_wipe(
    ctx: &Ctx,
    mount: &Path,
    passes: u32,
    fs: &dyn WipeFs,
    job: &Job,
    max_file: u64,
    on_pass_done: Option<&dyn Fn(&freespace::PassDone<'_>)>,
) -> Result<FillReport> {
    // Exclusions must not be able to keep the filler files (and the disk full) around.
    let safety = Arc::new(Safety::new(&ctx.env, ExcludeSet::empty()));
    fill_free_space(&FillParams {
        mount,
        safety,
        candidates: freespace::candidate_locations(ctx, mount),
        patterns: overwrite_passes(passes),
        fs,
        job,
        max_file,
        on_pass_done,
    })
}

fn wipe_free_space(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let p: FreeParams = serde_json::from_value(params)?;
    if !valid_passes(p.passes) {
        return Err(ApiError::invalid_params(
            "`passes` must be one of 1, 3, 7 or 35",
        ));
    }
    let want = key_of_path(Path::new(&p.mount));
    let vol = list_volumes(ctx)
        .into_iter()
        .find(|v| key_of_path(Path::new(&v.mount)) == want)
        .ok_or_else(|| ApiError::invalid_params(format!("{} is not a mounted drive", p.mount)))?;
    let mount = PathBuf::from(&vol.mount);
    let report = run_free_space_wipe(ctx, &mount, p.passes, &RealFs, job, MAX_FILE_BYTES, None)?;
    Ok(report_json(&mount, p.passes, &report))
}

// ---------------------------------------------------------------- wipe_drive

fn wipe_drive(ctx: &Ctx, params: Value, job: &Job) -> Result<Value> {
    drive::wipe_drive(ctx, params, job, &RealBackend)
}
