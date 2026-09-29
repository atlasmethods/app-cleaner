/** Planned methods for the `health` feature. Request/response types are added when it is implemented. */
export const HEALTH_METHODS = [
  'health.analyze',
  'health.fix',
] as const;

export type HealthMethod = (typeof HEALTH_METHODS)[number];
