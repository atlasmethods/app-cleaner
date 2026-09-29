/** Mirrors `sweep_core::features::driver_updater` (serde camelCase). */
export const DRIVER_UPDATER_METHODS = [
  'driver_updater.scan',
  'driver_updater.backup',
  'driver_updater.update',
] as const;

export type DriverUpdaterMethod = (typeof DRIVER_UPDATER_METHODS)[number];

export type DriverSource = 'fwupd' | 'ubuntu-drivers' | 'windows-update' | 'macos';

export interface DriverEntry {
  id: string;
  deviceName: string;
  currentVersion?: string;
  newVersion?: string;
  vendor?: string;
  source: DriverSource;
  description: string;
  rebootRequired?: boolean;
}

export interface DriverResult {
  id: string;
  deviceName: string;
  ok: boolean;
  exitCode?: number;
  message: string;
  rebootRequired: boolean;
}

export interface DriverUpdateReport {
  results: DriverResult[];
  rebootRequired: boolean;
  succeeded: number;
  failed: number;
  backupPath: string | null;
}

export interface BackupResult {
  ok: boolean;
  path: string;
  exitCode: number;
  message: string;
}
