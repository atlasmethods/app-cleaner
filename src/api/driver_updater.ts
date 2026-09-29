/** Planned methods for the `driver_updater` feature. Request/response types are added when it is implemented. */
export const DRIVER_UPDATER_METHODS = [
  'driver_updater.scan',
  'driver_updater.update',
] as const;

export type DriverUpdaterMethod = (typeof DRIVER_UPDATER_METHODS)[number];
