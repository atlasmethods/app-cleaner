/** Mirrors `sweep_core::features::wiper` (serde camelCase). */
export const WIPER_METHODS = [
  'wiper.list_drives',
  'wiper.list_devices',
  'wiper.wipe_free_space',
  'wiper.wipe_drive',
] as const;

export type WiperMethod = (typeof WIPER_METHODS)[number];

export type Passes = 1 | 3 | 7 | 35;

export interface WiperDrive {
  name: string;
  mount: string;
  fs: string;
  total: number;
  available: number;
  removable: boolean;
  isSystem: boolean;
  device?: string;
  wholeDisk?: string;
}

export interface WiperDevice {
  device: string;
  name: string;
  sizeBytes: number;
  removable: boolean;
  model: string | null;
  partitions: { name: string; device: string; sizeBytes: number; holders: string[] }[];
  mounts: string[];
  swap: boolean;
  holders: string[];
  isSystem: boolean;
}

export interface DeviceListing {
  supported: boolean;
  devices: WiperDevice[];
}

export interface FreeSpaceReport {
  mount: string;
  passes: number;
  filesWritten: number;
  bytesPerPass: number;
  bytesWritten: number;
  freeBefore: number | null;
  freeAfter: number | null;
  location: string;
  staleRemoved: number;
  durationMs: number;
}

export interface DriveWipeReport {
  device: string;
  passes: number;
  bytes?: number;
  durationMs: number;
}
