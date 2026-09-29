/** Planned methods for the `smart_cleaning` feature. Request/response types are added when it is implemented. */
export const SMART_CLEANING_METHODS = [
  'smart_cleaning.get_config',
  'smart_cleaning.set_config',
  'smart_cleaning.check',
] as const;

export type SmartCleaningMethod = (typeof SMART_CLEANING_METHODS)[number];
