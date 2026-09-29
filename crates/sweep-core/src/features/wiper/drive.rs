//! Whole-drive wiping.
//!
//! This is the most dangerous operation in the app, so every refusal happens BEFORE the
//! device is opened, and the checks are layered:
//!
//! 1. `confirm` must equal the device path exactly (the user typed it);
//! 2. the device must be a whole physical disk that ClearSweep itself enumerated;
//! 3. it must not be the system drive (any partition mounted at `/`, `/boot`, `/usr`, ...);
//! 4. no partition may be mounted, used as swap, or held by LVM / dm-crypt / md;
//! 5. the process must be elevated;
//! 6. on Linux the device is opened with `O_EXCL`, which makes the kernel refuse a device
//!    that is in use even if the checks above missed something.
//!
//! Linux writes the pattern schedule itself. macOS delegates to `diskutil secureErase`
//! (and refuses the boot disk). Windows is not supported yet.

use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::time::Instant;

use rand::rngs::StdRng;
use serde_json::{json, Value};

use crate::ctx::{Ctx, Os};
use crate::error::{ApiError, Result};
use crate::features::secure_delete::{fill_chunk, overwrite_passes};
use crate::job::{Job, ProgressEvent};
use crate::safety::{io_message, key_of_path};

use super::devices::{self, BlockDevice, Usage};

const CHUNK: usize = 4 << 20;

/// Something we can write a whole drive through (a block device, or a regular file in tests).
pub trait DeviceIo: Write + Seek {
    fn sync(&mut self) -> io::Result<()>;
}

impl DeviceIo for File {
    fn sync(&mut self) -> io::Result<()> {
        self.sync_all()
    }
}

/// A whole disk together with what is using it.
#[derive(Debug, Clone)]
pub struct DeviceState {
    pub device: BlockDevice,
    pub usage: Usage,
    pub is_system: bool,
    /// False when the list of mounted filesystems could not be read: nothing can be
    /// ruled out, so the drive is not wiped.
    pub mounts_known: bool,
}

/// The parts of a wipe that touch the real machine; tests substitute their own.
pub trait DriveBackend: Send + Sync {
    fn devices(&self, ctx: &Ctx) -> Vec<DeviceState>;
    fn is_elevated(&self) -> bool;
    fn open(&self, device: &str) -> io::Result<Box<dyn DeviceIo>>;
}

pub struct RealBackend;

/// Logical mount points that belong to the operating system on Linux.
const LINUX_SYSTEM_MOUNTS: &[&str] = &["/", "/boot", "/boot/efi", "/usr", "/var", "/etc", "/opt"];

pub fn linux_system_mount(mount: &str) -> bool {
    LINUX_SYSTEM_MOUNTS
        .iter()
        .any(|m| key_of_path(std::path::Path::new(m)) == key_of_path(std::path::Path::new(mount)))
}

impl DriveBackend for RealBackend {
    fn devices(&self, ctx: &Ctx) -> Vec<DeviceState> {
        let devs = devices::list_block_devices(&ctx.env);
        let mounts = devices::read_mounts(&ctx.env);
        let mountinfo = devices::read_mountinfo(&ctx.env);
        let swaps = devices::read_swaps(&ctx.env);
        let mounts_known = !mounts.is_empty() || !mountinfo.is_empty();
        devs.into_iter()
            .map(|d| {
                let usage = devices::usage_of(&d, &mounts, &mountinfo, &swaps);
                let is_system = usage.mounts.iter().any(|m| linux_system_mount(m));
                DeviceState {
                    device: d,
                    usage,
                    is_system,
                    mounts_known,
                }
            })
            .collect()
    }

    fn is_elevated(&self) -> bool {
        crate::elevate::is_elevated()
    }

    #[cfg(target_os = "linux")]
    fn open(&self, device: &str) -> io::Result<Box<dyn DeviceIo>> {
        use std::os::unix::fs::OpenOptionsExt;
        let f = OpenOptions::new()
            .write(true)
            // O_EXCL on a block device: fail with EBUSY if it, or a partition, is in use.
            .custom_flags(libc::O_EXCL)
            .open(device)?;
        Ok(Box::new(f))
    }

    #[cfg(not(target_os = "linux"))]
    fn open(&self, device: &str) -> io::Result<Box<dyn DeviceIo>> {
        Ok(Box::new(OpenOptions::new().write(true).open(device)?))
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WipeDriveParams {
    pub device: String,
    pub passes: u32,
    #[serde(default)]
    pub confirm: String,
}

pub fn valid_passes(p: u32) -> bool {
    matches!(p, 1 | 3 | 7 | 35)
}

fn refuse(msg: impl Into<String>) -> ApiError {
    ApiError::permission_denied(msg)
}

/// `wiper.wipe_drive` with an injectable backend.
pub fn wipe_drive(
    ctx: &Ctx,
    params: Value,
    job: &Job,
    backend: &dyn DriveBackend,
) -> Result<Value> {
    let p: WipeDriveParams = serde_json::from_value(params)?;
    if !valid_passes(p.passes) {
        return Err(ApiError::invalid_params(
            "`passes` must be one of 1, 3, 7 or 35",
        ));
    }
    match ctx.env.os {
        Os::Windows => Err(ApiError::unsupported(
            "Wiping a whole drive is not supported on Windows yet. Wipe the free space instead, or use `diskpart` > `clean all` from an administrator command prompt.",
        )),
        Os::MacOs => wipe_drive_macos(ctx, &p, job, backend),
        Os::Linux => wipe_drive_linux(ctx, &p, job, backend),
    }
}

fn confirm_check(p: &WipeDriveParams) -> Result<()> {
    if p.confirm != p.device {
        return Err(ApiError::invalid_params(
            "confirmation does not match: type the drive's device path exactly as shown",
        ));
    }
    Ok(())
}

fn wipe_drive_linux(
    ctx: &Ctx,
    p: &WipeDriveParams,
    job: &Job,
    backend: &dyn DriveBackend,
) -> Result<Value> {
    confirm_check(p)?;
    let state = backend
        .devices(ctx)
        .into_iter()
        .find(|d| d.device.device == p.device)
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "{} is not a whole physical disk that ClearSweep can wipe",
                p.device
            ))
        })?;
    if state.is_system {
        return Err(refuse(format!(
            "{} holds the operating system and can never be wiped",
            p.device
        )));
    }
    if !state.mounts_known {
        return Err(refuse(
            "cannot read the list of mounted filesystems, so it cannot be checked that the drive is unused",
        ));
    }
    if state.usage.in_use() {
        let mut why = Vec::new();
        if !state.usage.mounts.is_empty() {
            why.push(format!("mounted at {}", state.usage.mounts.join(", ")));
        }
        if state.usage.swap {
            why.push("used as swap".to_string());
        }
        if !state.usage.holders.is_empty() {
            why.push(format!("in use by {}", state.usage.holders.join(", ")));
        }
        return Err(refuse(format!(
            "{} is {}; unmount / release it first",
            p.device,
            why.join("; ")
        )));
    }
    if !backend.is_elevated() {
        return Err(refuse(
            "Administrator rights are required to wipe a whole drive. Start ClearSweep with sudo or pkexec.",
        ));
    }
    job.check_cancelled()?;
    let mut dev = backend.open(&p.device).map_err(|e| {
        if e.raw_os_error() == busy_code() {
            refuse(format!("{} is busy (in use by the system)", p.device))
        } else {
            ApiError::from(io::Error::new(
                e.kind(),
                format!("{}: {}", p.device, io_message(&e)),
            ))
        }
    })?;
    let start = Instant::now();
    let size = overwrite_device(dev.as_mut(), p.passes, job)?;
    Ok(json!({
        "device": p.device,
        "passes": p.passes,
        "bytes": size,
        "durationMs": start.elapsed().as_millis() as u64,
    }))
}

fn busy_code() -> Option<i32> {
    #[cfg(unix)]
    {
        Some(libc::EBUSY)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Write every pass over the whole device, syncing after each. Returns the device size.
pub fn overwrite_device(dev: &mut dyn DeviceIo, passes: u32, job: &Job) -> Result<u64> {
    let size = dev.seek(SeekFrom::End(0))?;
    if size == 0 {
        return Err(ApiError::io("the device reports a size of 0 bytes"));
    }
    let patterns = overwrite_passes(passes);
    let n_pass = patterns.len();
    let mut rng: StdRng = rand::make_rng();
    let mut buf = vec![0u8; CHUNK];
    let mut last = Instant::now();
    for (i, pat) in patterns.iter().enumerate() {
        dev.seek(SeekFrom::Start(0))?;
        let mut done = 0u64;
        while done < size {
            job.check_cancelled()?;
            let n = ((size - done).min(buf.len() as u64)) as usize;
            fill_chunk(&mut buf[..n], pat, done, &mut rng);
            dev.write_all(&buf[..n])?;
            done += n as u64;
            if last.elapsed().as_millis() >= 100 {
                last = Instant::now();
                let frac = (i as f64 + done as f64 / size as f64) / n_pass as f64;
                job.progress(
                    ProgressEvent::new("wipe")
                        .fraction(frac)
                        .message(format!("Pass {} of {n_pass}", i + 1)),
                );
            }
        }
        dev.flush()?;
        dev.sync()?;
    }
    Ok(size)
}

// ---------------------------------------------------------------- macOS

/// `disk12s3` / `disk12` -> `disk12`.
pub fn disk_base(s: &str) -> Option<String> {
    let rest = s.trim().trim_start_matches("/dev/").strip_prefix("disk")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then(|| format!("disk{digits}"))
}

/// Disks that make up the boot volume, from the text of `diskutil info /`.
pub fn boot_disks(info: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in info.lines() {
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        let label = label.trim();
        if matches!(
            label,
            "Part of Whole" | "APFS Physical Store" | "Device Identifier"
        ) {
            if let Some(b) = disk_base(value) {
                if !out.contains(&b) {
                    out.push(b);
                }
            }
        }
    }
    out
}

fn info_flag(info: &str, label: &str) -> Option<bool> {
    info.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == label).then(|| v.trim().eq_ignore_ascii_case("yes"))
    })
}

fn secure_erase_level(passes: u32) -> &'static str {
    // diskutil: 0 zero-fill, 1 random, 2 US DoD 7-pass, 3 Gutmann 35-pass, 4 US DoD 3-pass.
    match passes {
        3 => "4",
        7 => "2",
        35 => "3",
        _ => "1",
    }
}

fn wipe_drive_macos(
    ctx: &Ctx,
    p: &WipeDriveParams,
    job: &Job,
    backend: &dyn DriveBackend,
) -> Result<Value> {
    confirm_check(p)?;
    let base = disk_base(&p.device)
        .ok_or_else(|| ApiError::invalid_params("the device must look like /dev/diskN"))?;
    if p.device != format!("/dev/{base}") {
        return Err(ApiError::invalid_params(
            "give the whole disk (/dev/diskN), not a partition",
        ));
    }
    // Fail closed: if the boot disk cannot be determined, nothing is erased.
    let boot = ctx
        .runner
        .run("diskutil", &["info", "/"])
        .map_err(|e| refuse(format!("cannot determine the boot disk: {}", e.message)))?;
    if !boot.success() {
        return Err(refuse(
            "cannot determine the boot disk (diskutil info / failed)",
        ));
    }
    let boots = boot_disks(&boot.stdout);
    if boots.is_empty() || boots.contains(&base) {
        return Err(refuse(format!(
            "{} is (or may be) the disk macOS boots from and can never be wiped",
            p.device
        )));
    }
    let info = ctx
        .runner
        .run("diskutil", &["info", &p.device])
        .map_err(|e| ApiError::not_found(format!("{}: {}", p.device, e.message)))?;
    if !info.success() {
        return Err(ApiError::not_found(format!("{} does not exist", p.device)));
    }
    if info_flag(&info.stdout, "Whole") != Some(true) {
        return Err(ApiError::invalid_params(format!(
            "{} is not a whole disk",
            p.device
        )));
    }
    if !backend.is_elevated() {
        return Err(refuse(
            "Administrator rights are required to wipe a whole drive. Start ClearSweep with sudo.",
        ));
    }
    job.check_cancelled()?;
    let start = Instant::now();
    // A normal (not forced) unmount: fails if something still uses a volume.
    let un = ctx.runner.run("diskutil", &["unmountDisk", &p.device])?;
    if !un.success() {
        return Err(refuse(format!(
            "could not unmount {}: {}",
            p.device,
            un.stderr.trim()
        )));
    }
    let level = secure_erase_level(p.passes);
    job.progress(
        ProgressEvent::new("wipe").message("Erasing with diskutil; this cannot be interrupted"),
    );
    let er = ctx
        .runner
        .run("diskutil", &["secureErase", level, &p.device])?;
    if !er.success() {
        return Err(ApiError::io(format!(
            "diskutil secureErase failed: {}",
            er.stderr.trim()
        )));
    }
    Ok(json!({
        "device": p.device,
        "passes": p.passes,
        "durationMs": start.elapsed().as_millis() as u64,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_base_parsing() {
        assert_eq!(disk_base("disk3s1s1").as_deref(), Some("disk3"));
        assert_eq!(disk_base("/dev/disk12").as_deref(), Some("disk12"));
        assert_eq!(disk_base(" disk0s2 ").as_deref(), Some("disk0"));
        assert_eq!(disk_base("sda"), None);
        assert_eq!(disk_base("disk"), None);
    }

    #[test]
    fn boot_disks_include_physical_stores() {
        let info = "   Device Identifier:         disk3s1s1\n   Part of Whole:             disk3\n   APFS Physical Store:       disk0s2\n   Volume Name:               Macintosh HD\n";
        assert_eq!(boot_disks(info), vec!["disk3", "disk0"]);
    }

    #[test]
    fn linux_system_mounts() {
        for m in ["/", "/boot", "/boot/efi", "/usr", "/var", "/etc/"] {
            assert!(linux_system_mount(m), "{m}");
        }
        for m in ["/home", "/mnt/usb", "/media/x", "/data", "/boot2"] {
            assert!(!linux_system_mount(m), "{m}");
        }
    }
}
