/** Mirrors `sweep_core::features::uninstall` (serde camelCase). */
export const UNINSTALL_METHODS = [
  'uninstall.list',
  'uninstall.run',
  'uninstall.repair',
  'uninstall.remove_entry',
  'uninstall.rename_entry',
  'uninstall.leftovers',
  'uninstall.remove_leftovers',
] as const;

export type UninstallMethod = (typeof UNINSTALL_METHODS)[number];

export type AppSource =
  | 'dpkg'
  | 'rpm'
  | 'pacman'
  | 'flatpak'
  | 'snap'
  | 'appimage'
  | 'windows'
  | 'macapp'
  | 'brew';

export interface AppEntry {
  id: string;
  name: string;
  version: string;
  publisher: string;
  /** YYYY-MM-DD */
  installDate?: string;
  sizeBytes?: number;
  source: AppSource;
  uninstallable: boolean;
  canRepair: boolean;
  canModify: boolean;
  isSystem: boolean;
  icon?: string;
}

export interface Leftover {
  path: string;
  sizeBytes: number;
  kind: 'config' | 'cache' | 'data' | 'launcher' | 'prefs' | 'logs';
}

export interface RunResult {
  id: string;
  name: string;
  ok: boolean;
  exitCode?: number;
  message: string;
  alsoRemoves: string[];
  needsForce: boolean;
  rebootRequired: boolean;
  leftovers: Leftover[];
  bundleId?: string;
  /** remove_entry / rename_entry */
  backupPath?: string;
  newName?: string;
}

export interface RemoveLeftoversResult {
  results: { path: string; ok: boolean; error?: string; bytes: number }[];
  totalBytes: number;
}
