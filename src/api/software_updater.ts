/** Mirrors `sweep_core::features::software_updater` (serde camelCase). */
export const SOFTWARE_UPDATER_METHODS = [
  'software_updater.list',
  'software_updater.update',
  'software_updater.update_all',
  'software_updater.set_ignored',
] as const;

export type SoftwareUpdaterMethod = (typeof SOFTWARE_UPDATER_METHODS)[number];

export type UpdateSource =
  | 'apt'
  | 'dnf'
  | 'pacman'
  | 'flatpak'
  | 'snap'
  | 'winget'
  | 'brew'
  | 'brew-cask'
  | 'macos';

export interface UpdateEntry {
  id: string;
  name: string;
  currentVersion: string;
  newVersion: string;
  source: UpdateSource;
  ignored: boolean;
  security?: boolean;
}

export interface UpdateResult {
  id: string;
  name: string;
  ok: boolean;
  exitCode?: number;
  message: string;
}

export interface UpdateReport {
  results: UpdateResult[];
  succeeded: number;
  failed: number;
}
