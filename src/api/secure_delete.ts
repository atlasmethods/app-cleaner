/** Planned methods for the `secure_delete` feature. Request/response types are added when it is implemented. */
export const SECURE_DELETE_METHODS = [
  'secure_delete.delete',
] as const;

export type SecureDeleteMethod = (typeof SECURE_DELETE_METHODS)[number];
