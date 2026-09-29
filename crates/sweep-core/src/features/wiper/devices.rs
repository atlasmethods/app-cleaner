//! Block devices and mounts read from sysfs / `/proc` (all paths go through
//! [`Env::sys_path`], so tests can build a fake machine).

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ctx::Env;

/// Device name prefixes of real disks. Loop, RAM, zram, device-mapper, md and optical
/// devices are deliberately not listed: they are never offered for a whole-drive wipe.
const DISK_PREFIXES: &[&str] = &["sd", "hd", "vd", "xvd", "nvme", "mmcblk"];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Partition {
    /// `major:minor` from sysfs; how mounts are matched when the mount source is not a
    /// device path (`/dev/root`).
    #[serde(skip)]
    pub majmin: Option<String>,
    pub name: String,
    pub device: String,
    pub size_bytes: u64,
    /// Kernel devices stacked on top of this partition (LVM, dm-crypt, md).
    pub holders: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlockDevice {
    #[serde(skip)]
    pub majmin: Option<String>,
    pub name: String,
    /// `/dev/sdb` (under the environment's root).
    pub device: String,
    pub size_bytes: u64,
    pub removable: bool,
    pub model: Option<String>,
    pub partitions: Vec<Partition>,
    pub holders: Vec<String>,
}

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn read_u64(p: &Path) -> Option<u64> {
    read_trim(p)?.parse().ok()
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// Whole disks known to sysfs, with their partitions.
pub fn list_block_devices(env: &Env) -> Vec<BlockDevice> {
    let base = env.sys_path("/sys/block");
    let mut out = Vec::new();
    for name in names_in(&base) {
        if !DISK_PREFIXES.iter().any(|p| name.starts_with(p)) {
            continue;
        }
        let dir = base.join(&name);
        let sectors = read_u64(&dir.join("size")).unwrap_or(0);
        let partitions: Vec<Partition> = names_in(&dir)
            .into_iter()
            .filter(|n| dir.join(n).join("partition").exists())
            .map(|n| {
                let pdir = dir.join(&n);
                Partition {
                    majmin: read_trim(&pdir.join("dev")),
                    device: env
                        .sys_path(format!("/dev/{n}"))
                        .to_string_lossy()
                        .into_owned(),
                    size_bytes: read_u64(&pdir.join("size")).unwrap_or(0) * 512,
                    holders: names_in(&pdir.join("holders")),
                    name: n,
                }
            })
            .collect();
        out.push(BlockDevice {
            majmin: read_trim(&dir.join("dev")),
            device: env
                .sys_path(format!("/dev/{name}"))
                .to_string_lossy()
                .into_owned(),
            size_bytes: sectors * 512,
            removable: read_u64(&dir.join("removable")) == Some(1),
            model: read_trim(&dir.join("device").join("model")).filter(|m| !m.is_empty()),
            holders: names_in(&dir.join("holders")),
            partitions,
            name,
        });
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    pub source: String,
    pub mount: String,
    pub fs: String,
}

/// `\040` style octal escapes used by `/proc/mounts` (space, tab, newline, backslash).
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v = (b[i + 1] - b'0') as u32 * 64
                + (b[i + 2] - b'0') as u32 * 8
                + (b[i + 3] - b'0') as u32;
            out.push(v as u8);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_mounts(text: &str) -> Vec<MountEntry> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some(MountEntry {
                source: unescape(it.next()?),
                mount: unescape(it.next()?),
                fs: it.next()?.to_string(),
            })
        })
        .collect()
}

/// One line of `/proc/self/mountinfo`, reduced to what matching needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfo {
    /// `major:minor` of the device backing the mount.
    pub majmin: String,
    pub mount: String,
    pub fs: String,
}

/// `36 35 98:0 /mnt1 /mnt2 rw,noatime master:1 - ext3 /dev/root rw,errors=continue`
pub fn parse_mountinfo(text: &str) -> Vec<MountInfo> {
    text.lines()
        .filter_map(|l| {
            let (pre, post) = l.split_once(" - ")?;
            let mut a = pre.split_whitespace();
            let majmin = a.nth(2)?.to_string();
            let mount = unescape(a.nth(1)?);
            let fs = post.split_whitespace().next()?.to_string();
            Some(MountInfo { majmin, mount, fs })
        })
        .collect()
}

pub fn read_mountinfo(env: &Env) -> Vec<MountInfo> {
    fs::read_to_string(env.sys_path("/proc/self/mountinfo"))
        .map(|t| parse_mountinfo(&t))
        .unwrap_or_default()
}

/// Devices listed in `/proc/swaps` (first column, header skipped).
pub fn parse_swaps(text: &str) -> Vec<String> {
    text.lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().next().map(unescape))
        .collect()
}

pub fn read_mounts(env: &Env) -> Vec<MountEntry> {
    fs::read_to_string(env.sys_path("/proc/mounts"))
        .map(|t| parse_mounts(&t))
        .unwrap_or_default()
}

pub fn read_swaps(env: &Env) -> Vec<String> {
    fs::read_to_string(env.sys_path("/proc/swaps"))
        .map(|t| parse_swaps(&t))
        .unwrap_or_default()
}

/// Resolve a `/dev/disk/by-uuid/...` style source to the device node it points at.
fn resolve(source: &str) -> PathBuf {
    fs::canonicalize(source).unwrap_or_else(|_| PathBuf::from(source))
}

/// Does mount/swap `source` refer to the device node `dev`?
pub fn source_is(source: &str, dev: &str) -> bool {
    source == dev || resolve(source) == resolve(dev)
}

/// Whole-disk device path holding the partition/device `source` (`/dev/sda1` -> `/dev/sda`).
pub fn whole_disk_of(devices: &[BlockDevice], source: &str) -> Option<String> {
    devices
        .iter()
        .find(|d| {
            source_is(source, &d.device)
                || d.partitions.iter().any(|p| source_is(source, &p.device))
        })
        .map(|d| d.device.clone())
}

/// What is using a device right now.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Mount points of the disk or any of its partitions.
    pub mounts: Vec<String>,
    /// Swap is active on the disk or one of its partitions.
    pub swap: bool,
    /// Stacked kernel devices (LVM, dm-crypt, md RAID) using it.
    pub holders: Vec<String>,
}

impl Usage {
    pub fn in_use(&self) -> bool {
        !self.mounts.is_empty() || self.swap || !self.holders.is_empty()
    }
}

/// What uses `dev` (the disk itself or any of its partitions). Mounts are matched by
/// device path AND by `major:minor` (`/proc/self/mountinfo`), so a root filesystem
/// reported as `/dev/root` or through an unresolvable alias is still recognised.
pub fn usage_of(
    dev: &BlockDevice,
    mounts: &[MountEntry],
    mountinfo: &[MountInfo],
    swaps: &[String],
) -> Usage {
    let nodes: Vec<&str> = std::iter::once(dev.device.as_str())
        .chain(dev.partitions.iter().map(|p| p.device.as_str()))
        .collect();
    let mut u = Usage::default();
    for m in mounts {
        if nodes.iter().any(|n| source_is(&m.source, n)) {
            u.mounts.push(m.mount.clone());
        }
    }
    let numbers: Vec<&str> = std::iter::once(dev.majmin.as_deref())
        .chain(dev.partitions.iter().map(|p| p.majmin.as_deref()))
        .flatten()
        .collect();
    for m in mountinfo {
        if numbers.contains(&m.majmin.as_str()) && !u.mounts.contains(&m.mount) {
            u.mounts.push(m.mount.clone());
        }
    }
    u.swap = swaps.iter().any(|s| nodes.iter().any(|n| source_is(s, n)));
    u.holders.extend(dev.holders.iter().cloned());
    for p in &dev.partitions {
        u.holders.extend(p.holders.iter().cloned());
    }
    u
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_parses_with_optional_fields_and_escapes() {
        let m = parse_mountinfo(
            "36 35 98:0 /mnt1 /mnt2 rw,noatime master:1 - ext3 /dev/root rw\n25 1 259:2 / /media/my\\040disk rw - vfat /dev/nvme0n1p2 rw\n",
        );
        assert_eq!(m.len(), 2);
        assert_eq!(
            m[0],
            MountInfo {
                majmin: "98:0".into(),
                mount: "/mnt2".into(),
                fs: "ext3".into()
            }
        );
        assert_eq!(m[1].mount, "/media/my disk");
        assert_eq!(m[1].majmin, "259:2");
        assert!(parse_mountinfo("garbage without separator").is_empty());
    }

    #[test]
    fn mounts_and_swaps_parse() {
        let m = parse_mounts(
            "/dev/sda1 / ext4 rw 0 0\n/dev/sdb1 /mnt/my\\040disk vfat rw 0 0\nproc /proc proc rw 0 0\n",
        );
        assert_eq!(m.len(), 3);
        assert_eq!(m[1].mount, "/mnt/my disk");
        assert_eq!(m[1].fs, "vfat");
        assert_eq!(
            parse_swaps("Filename\tType\tSize\tUsed\tPriority\n/dev/sda2 partition 100 0 -2\n"),
            vec!["/dev/sda2"]
        );
    }
}
