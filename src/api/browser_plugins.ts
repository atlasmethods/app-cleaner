/** Planned methods for the `browser_plugins` feature. Request/response types are added when it is implemented. */
export const BROWSER_PLUGINS_METHODS = [
  'browser_plugins.list',
  'browser_plugins.set_enabled',
  'browser_plugins.remove',
] as const;

export type BrowserPluginsMethod = (typeof BROWSER_PLUGINS_METHODS)[number];
