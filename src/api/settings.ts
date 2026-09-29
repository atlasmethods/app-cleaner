/** Mirrors `sweep_core::features::settings` (serde camelCase). */
export const SETTINGS_METHODS = ['settings.get', 'settings.set', 'settings.reset'] as const;

export type SettingsMethod = (typeof SETTINGS_METHODS)[number];

export type Theme = 'system' | 'light' | 'dark';
export type CloseBrowsers = 'ask' | 'always' | 'skip';
export type Passes = 1 | 3 | 7 | 35;

export interface SecureDeleteSettings {
  enabled: boolean;
  passes: Passes;
}

export interface IncludeEntry {
  id: string;
  path: string;
  recursive: boolean;
  mask: string;
  removeEmptyDirs: boolean;
}

export interface ExcludeEntry {
  id: string;
  pattern: string;
}

export interface SmartSettings {
  enabled: boolean;
  thresholdMb: number;
  cleanOnBrowserClose: string[];
  autoClean: boolean;
  notify: boolean;
}

export interface Settings {
  theme: Theme;
  secureDelete: SecureDeleteSettings;
  closeBrowsers: CloseBrowsers;
  tempMinAgeHours: number;
  include: IncludeEntry[];
  exclude: ExcludeEntry[];
  cookieKeep: string[];
  /** null = every rule's own default */
  selectedRules: string[] | null;
  smart: SmartSettings;
  runAtStartup: boolean;
  language: string;
}

/** `settings.set` takes an RFC 7386 merge patch of the above (null deletes a key). */
export type SettingsPatch = { [K in keyof Settings]?: Partial<Settings[K]> | Settings[K] | null };
