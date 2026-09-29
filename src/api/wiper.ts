/** Planned methods for the `wiper` feature. Request/response types are added when it is implemented. */
export const WIPER_METHODS = [
  'wiper.list_drives',
  'wiper.wipe_free_space',
] as const;

export type WiperMethod = (typeof WIPER_METHODS)[number];
