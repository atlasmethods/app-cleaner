//! Mounted volumes, shared by `disk_analyzer.list_drives` and `wiper.list_drives`.

use ::sysinfo as si;
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::ctx::{Ctx, Os};
use crate::safety::is_within;

/// One mounted, real (non-pseudo) volume.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    /// Volume label / device name as reported by the OS (may be empty).
    pub name: String,
    pub mount: String,
    pub fs: String,
    pub total: u64,
    pub available: u64,
    pub removable: bool,
}

/// Filesystems that never hold user data.
const PSEUDO_FS: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "pstore",
    "securityfs",
    "debugfs",
    "tracefs",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "bpf",
    "autofs",
    "binfmt_misc",
    "rpc_pipefs",
    "nsfs",
    "efivarfs",
    "selinuxfs",
    "squashfs",
    "devfs",
    "fdescfs",
    "binder",
    "kernfs",
    "fuse.gvfsd-fuse",
    "fuse.portal",
    "fuse.lxcfs",
    "fuse.snapfuse",
    "fuse.sshfs-dummy",
];

/// Where RAM-backed filesystems belong to the system rather than to the user.
const SYSTEM_TMPFS_PREFIXES: &[&str] = &[
    "/dev", "/run", "/sys", "/proc", "/tmp", "/var", "/snap", "/boot", "/mnt/wsl", "/tmpfs",
];

/// Should this mount be hidden from drive pickers?
///
/// Excluded: kernel pseudo filesystems (`proc`, `sysfs`, cgroups, ...), read-only snap
/// images (`squashfs`), and RAM filesystems (`tmpfs`, `ramfs`) mounted at system
/// locations (`/dev/shm`, `/run`, `/tmp`, ...). A tmpfs the user mounted somewhere else
/// (say `/mnt/scratch`) is kept. `overlay` is kept: it is the root filesystem of
/// containers. On macOS the helper volumes below `/System/Volumes` are hidden except
/// `Data`, which holds the user's files.
pub fn is_pseudo(fs: &str, mount: &Path, os: Os) -> bool {
    let f = fs.to_ascii_lowercase();
    if f.starts_with("cgroup") || PSEUDO_FS.contains(&f.as_str()) {
        return true;
    }
    if os == Os::Windows {
        return false;
    }
    let under = |p: &str| is_within(mount, Path::new(p));
    if os == Os::MacOs {
        if under("/System/Volumes") && !mount.ends_with("Data") {
            return true;
        }
        if under("/dev") {
            return true;
        }
    }
    if (f == "tmpfs" || f == "ramfs") && SYSTEM_TMPFS_PREFIXES.iter().any(|p| under(p)) {
        return true;
    }
    under("/proc") || under("/sys") || (os == Os::Linux && under("/dev"))
}

/// Real volumes of this machine, sorted by mount point.
pub fn list_volumes(ctx: &Ctx) -> Vec<Volume> {
    let disks = si::Disks::new_with_refreshed_list();
    let mut out: Vec<Volume> = Vec::new();
    for d in disks.list() {
        let mount: PathBuf = d.mount_point().to_path_buf();
        let fs = d.file_system().to_string_lossy().into_owned();
        // Bind-mounted single files (Docker's /etc/hosts) and vanished mounts.
        if !mount.is_dir() || d.total_space() == 0 || is_pseudo(&fs, &mount, ctx.env.os) {
            continue;
        }
        let mount_s = mount.to_string_lossy().into_owned();
        if out.iter().any(|v| v.mount == mount_s) {
            continue;
        }
        out.push(Volume {
            name: d.name().to_string_lossy().into_owned(),
            mount: mount_s,
            fs,
            total: d.total_space(),
            available: d.available_space(),
            removable: d.is_removable(),
        });
    }
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pseudo_filesystems_are_hidden() {
        for (fs, mount) in [
            ("proc", "/proc"),
            ("sysfs", "/sys"),
            ("cgroup2", "/sys/fs/cgroup"),
            ("devtmpfs", "/dev"),
            ("devpts", "/dev/pts"),
            ("tmpfs", "/dev/shm"),
            ("tmpfs", "/run"),
            ("tmpfs", "/run/user/1000"),
            ("tmpfs", "/tmp"),
            ("squashfs", "/snap/core/1"),
            ("efivarfs", "/sys/firmware/efi/efivars"),
            ("fuse.gvfsd-fuse", "/run/user/1000/gvfs"),
        ] {
            assert!(is_pseudo(fs, Path::new(mount), Os::Linux), "{fs} {mount}");
        }
        for (fs, mount) in [
            ("ext4", "/"),
            ("btrfs", "/home"),
            ("vfat", "/boot/efi"),
            ("overlay", "/"),
            ("xfs", "/data"),
            ("ntfs3", "/mnt/win"),
            ("tmpfs", "/mnt/scratch"),
            ("fuseblk", "/media/usb"),
        ] {
            assert!(!is_pseudo(fs, Path::new(mount), Os::Linux), "{fs} {mount}");
        }
    }

    #[test]
    fn macos_helper_volumes_are_hidden_but_data_is_kept() {
        assert!(is_pseudo(
            "apfs",
            Path::new("/System/Volumes/VM"),
            Os::MacOs
        ));
        assert!(is_pseudo(
            "apfs",
            Path::new("/System/Volumes/Preboot"),
            Os::MacOs
        ));
        assert!(!is_pseudo(
            "apfs",
            Path::new("/System/Volumes/Data"),
            Os::MacOs
        ));
        assert!(!is_pseudo("apfs", Path::new("/"), Os::MacOs));
        assert!(!is_pseudo("exfat", Path::new("/Volumes/USB"), Os::MacOs));
        assert!(is_pseudo("devfs", Path::new("/dev"), Os::MacOs));
    }

    #[test]
    fn windows_drives_are_never_pseudo() {
        assert!(!is_pseudo("NTFS", Path::new("C:\\"), Os::Windows));
        assert!(!is_pseudo("exFAT", Path::new("E:\\"), Os::Windows));
    }

    #[test]
    fn real_listing_has_no_pseudo_entries() {
        let d = tempfile::tempdir().unwrap();
        let ctx = Ctx::test(d.path(), crate::runner::MockRunner::new());
        for v in list_volumes(&ctx) {
            assert!(v.total > 0);
            assert!(!is_pseudo(&v.fs, Path::new(&v.mount), ctx.env.os), "{v:?}");
        }
    }
}
