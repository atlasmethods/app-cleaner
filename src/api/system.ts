/** Mirrors `sweep_core::features::system`. */
export const SYSTEM_METHODS = ['system.app_info', 'system.open_data_dir'] as const;

export interface AppInfo {
  name: string;
  version: string;
  license: string;
  dataDir: string;
  os: 'linux' | 'windows' | 'macos';
}
