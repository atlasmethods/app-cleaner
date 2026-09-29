/** Planned methods for the `restore` feature. Request/response types are added when it is implemented. */
export const RESTORE_METHODS = [
  'restore.list_points',
  'restore.create_point',
  'restore.restore',
] as const;

export type RestoreMethod = (typeof RESTORE_METHODS)[number];
