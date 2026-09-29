/** Planned methods for the `startup` feature. Request/response types are added when it is implemented. */
export const STARTUP_METHODS = [
  'startup.list',
  'startup.set_enabled',
  'startup.remove',
] as const;

export type StartupMethod = (typeof STARTUP_METHODS)[number];
