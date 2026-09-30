/** Mirrors `sweep_core::features::browser_plugins` (serde camelCase). */
export const BROWSER_PLUGINS_METHODS = [
  'browser_plugins.list',
  'browser_plugins.set_enabled',
  'browser_plugins.remove',
] as const;

export type BrowserPluginsMethod = (typeof BROWSER_PLUGINS_METHODS)[number];

export type PluginType = 'extension' | 'theme' | 'app' | 'plugin' | 'dictionary' | 'locale';

export interface Plugin {
  /** `browser:profile:extensionId` */
  id: string;
  browser: string;
  browserLabel: string;
  profile: string;
  profileName?: string;
  extensionId: string;
  name: string;
  version: string;
  description: string;
  enabled: boolean;
  type: PluginType;
  installLocation: string;
  canDisable: boolean;
  canRemove: boolean;
  note?: string;
  /** The browser is running right now. */
  running: boolean;
}

export interface SetPluginResult {
  ok: boolean;
  plugin: Plugin;
  backupId?: string;
}

export interface RemovePluginResult {
  ok: boolean;
  backupId: string;
  backupPath: string;
  note?: string | null;
}
