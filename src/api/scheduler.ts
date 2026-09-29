/** Planned methods for the `scheduler` feature. Request/response types are added when it is implemented. */
export const SCHEDULER_METHODS = [
  'scheduler.list',
  'scheduler.add',
  'scheduler.remove',
  'scheduler.set_enabled',
] as const;

export type SchedulerMethod = (typeof SCHEDULER_METHODS)[number];
