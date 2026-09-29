/** Planned methods for the `registry_cleaner` feature. Request/response types are added when it is implemented. */
export const REGISTRY_CLEANER_METHODS = [
  'registry_cleaner.scan',
  'registry_cleaner.fix',
  'registry_cleaner.list_backups',
  'registry_cleaner.restore_backup',
] as const;

export type RegistryCleanerMethod = (typeof REGISTRY_CLEANER_METHODS)[number];
