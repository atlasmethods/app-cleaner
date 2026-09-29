/** Planned methods for the `duplicates` feature. Request/response types are added when it is implemented. */
export const DUPLICATES_METHODS = [
  'duplicates.scan',
  'duplicates.delete',
] as const;

export type DuplicatesMethod = (typeof DUPLICATES_METHODS)[number];
