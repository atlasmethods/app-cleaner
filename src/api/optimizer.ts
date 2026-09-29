/** Planned methods for the `optimizer` feature. Request/response types are added when it is implemented. */
export const OPTIMIZER_METHODS = [
  'optimizer.analyze',
  'optimizer.apply',
] as const;

export type OptimizerMethod = (typeof OPTIMIZER_METHODS)[number];
