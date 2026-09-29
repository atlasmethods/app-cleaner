/** Mirrors `sweep_core::features::sysinfo` (serde camelCase). */

export interface OsInfo {
  name: string;
  version: string;
  kernel: string;
  hostname: string;
  arch: string;
}

export interface CpuInfo {
  brand: string;
  /** logical cores */
  cores: number;
  /** average utilisation 0..100 */
  usage: number;
}

export interface MemoryInfo {
  /** bytes */
  total: number;
  used: number;
}

export interface DiskInfo {
  name: string;
  mount: string;
  /** bytes */
  total: number;
  available: number;
  fs: string;
}

export interface SysInfo {
  os: OsInfo;
  cpu: CpuInfo;
  memory: MemoryInfo;
  disks: DiskInfo[];
  uptimeSecs: number;
}

export const SYSINFO_GET = 'sysinfo.get';
