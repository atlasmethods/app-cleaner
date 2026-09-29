/** Planned methods for the `uninstall` feature. Request/response types are added when it is implemented. */
export const UNINSTALL_METHODS = [
  'uninstall.list',
  'uninstall.run',
  'uninstall.remove_entry',
] as const;

export type UninstallMethod = (typeof UNINSTALL_METHODS)[number];
