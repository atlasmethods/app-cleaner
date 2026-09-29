/** Planned methods for the `software_updater` feature. Request/response types are added when it is implemented. */
export const SOFTWARE_UPDATER_METHODS = [
  'software_updater.list',
  'software_updater.update',
  'software_updater.update_all',
] as const;

export type SoftwareUpdaterMethod = (typeof SOFTWARE_UPDATER_METHODS)[number];
