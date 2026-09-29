/** Planned methods for the `cleaner` feature. Request/response types are added when it is implemented. */
export const CLEANER_METHODS = [
  'cleaner.list_rules',
  'cleaner.analyze',
  'cleaner.clean',
] as const;

export type CleanerMethod = (typeof CLEANER_METHODS)[number];
