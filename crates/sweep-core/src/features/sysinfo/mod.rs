//! System information (fully implemented; the end-to-end example feature).
//!
//! Methods:
//! - `sysinfo.get`: OS, CPU, memory, disks and uptime snapshot.

use ::sysinfo as si;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::Registry;
use crate::ctx::Ctx;
use crate::error::Result;
use crate::job::Job;

pub const METHODS: &[&str] = &["sysinfo.get"];

pub fn register(r: &mut Registry) {
    r.add("sysinfo.get", get);
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SysInfo {
    pub os: OsInfo,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub disks: Vec<DiskInfo>,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OsInfo {
    pub name: String,
    pub version: String,
    pub kernel: String,
    pub hostname: String,
    pub arch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CpuInfo {
    pub brand: String,
    /// Logical cores.
    pub cores: u32,
    /// Average utilisation, 0..=100.
    pub usage: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryInfo {
    /// Bytes.
    pub total: u64,
    pub used: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiskInfo {
    pub name: String,
    pub mount: String,
    /// Bytes.
    pub total: u64,
    pub available: u64,
    pub fs: String,
}

/// Collect a snapshot. CPU usage needs two samples, so this takes ~200ms.
pub fn collect() -> SysInfo {
    let mut sys = si::System::new();
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    std::thread::sleep(si::MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_cpu_usage();

    let cpus = sys.cpus();
    let brand = cpus
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "Unknown CPU".to_string());

    let disks = si::Disks::new_with_refreshed_list();
    let mut disk_list: Vec<DiskInfo> = disks
        .list()
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| DiskInfo {
            name: d.name().to_string_lossy().into_owned(),
            mount: d.mount_point().to_string_lossy().into_owned(),
            total: d.total_space(),
            available: d.available_space(),
            fs: d.file_system().to_string_lossy().into_owned(),
        })
        .collect();
    disk_list.sort_by(|a, b| a.mount.cmp(&b.mount));

    SysInfo {
        os: OsInfo {
            name: si::System::name().unwrap_or_else(|| std::env::consts::OS.to_string()),
            version: si::System::long_os_version()
                .or_else(si::System::os_version)
                .unwrap_or_default(),
            kernel: si::System::kernel_version().unwrap_or_default(),
            hostname: si::System::host_name().unwrap_or_default(),
            arch: std::env::consts::ARCH.to_string(),
        },
        cpu: CpuInfo {
            brand,
            cores: cpus.len() as u32,
            usage: sys.global_cpu_usage().clamp(0.0, 100.0),
        },
        memory: MemoryInfo {
            total: sys.total_memory(),
            used: sys.used_memory(),
        },
        disks: disk_list,
        uptime_secs: si::System::uptime(),
    }
}

fn get(_ctx: &Ctx, _params: Value, _job: &Job) -> Result<Value> {
    Ok(serde_json::to_value(collect())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dispatch;
    use crate::runner::MockRunner;

    #[test]
    fn sysinfo_get_returns_sane_values() {
        let d = tempfile::tempdir().unwrap();
        let ctx = Ctx::test(d.path(), MockRunner::new());
        let v = dispatch(&ctx, "sysinfo.get", Value::Null, &Job::detached()).unwrap();
        // camelCase on the wire
        assert!(v.get("uptimeSecs").is_some());
        assert!(v["os"].get("hostname").is_some());
        let info: SysInfo = serde_json::from_value(v).unwrap();
        assert!(info.cpu.cores >= 1);
        assert!(!info.cpu.brand.is_empty());
        assert!((0.0..=100.0).contains(&info.cpu.usage));
        assert!(info.memory.total > 0);
        assert!(info.memory.used <= info.memory.total);
        assert!(!info.os.name.is_empty());
        for disk in &info.disks {
            assert!(disk.available <= disk.total, "{disk:?}");
        }
    }
}
