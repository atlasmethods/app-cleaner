import type { DriverEntry, DriverSource } from '../api/driver_updater';

const LABEL: Record<DriverSource, string> = {
  fwupd: 'Firmware',
  'ubuntu-drivers': 'Driver',
  'windows-update': 'Windows Update',
  macos: 'macOS',
};

export function driverSourceLabel(s: DriverSource): string {
  return LABEL[s];
}

/** "1.20.0 -> 1.22.0", "-> 22.1.0.4" or an empty string when neither version is known. */
export function driverVersions(d: Pick<DriverEntry, 'currentVersion' | 'newVersion'>): string {
  const cur = d.currentVersion?.trim() ?? '';
  const next = d.newVersion?.trim() ?? '';
  if (!cur && !next) return '';
  if (!cur) return `→ ${next}`;
  if (!next) return cur;
  return `${cur} → ${next}`;
}
