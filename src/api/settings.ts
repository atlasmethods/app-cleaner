/** Planned methods for the `settings` feature. Request/response types are added when it is implemented. */
export const SETTINGS_METHODS = [
  'settings.get',
  'settings.set',
] as const;

export type SettingsMethod = (typeof SETTINGS_METHODS)[number];
