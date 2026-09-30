/** Mirrors `sweep_core::features::scheduler` (serde camelCase). */
export const SCHEDULER_METHODS = [
  'scheduler.list',
  'scheduler.add',
  'scheduler.update',
  'scheduler.remove',
  'scheduler.set_enabled',
  'scheduler.run_now',
  'scheduler.backend',
] as const;

export type SchedulerMethod = (typeof SCHEDULER_METHODS)[number];

export type Frequency = 'hourly' | 'daily' | 'weekly' | 'monthly' | 'on_login';

export interface ScheduleAction {
  kind: 'clean';
  /** null = the rules enabled in Settings when it runs */
  rules: string[] | null;
}

export interface LastResult {
  ok: boolean;
  totalBytes: number;
  totalFiles: number;
  message?: string;
}

export interface Schedule {
  id: string;
  name: string;
  enabled: boolean;
  frequency: Frequency;
  /** HH:MM, 24 hours (for hourly only the minutes count) */
  time: string;
  /** ISO weekdays: 1 = Monday .. 7 = Sunday */
  weekdays: number[];
  /** 1..28 */
  dayOfMonth: number;
  action: ScheduleAction;
  createdAt: string;
  lastRun?: string;
  lastResult?: LastResult;
}

/** Body of `scheduler.add` (and, with `id`, `scheduler.update`). */
export interface ScheduleInput {
  name: string;
  enabled: boolean;
  frequency: Frequency;
  time: string;
  weekdays: number[];
  dayOfMonth: number;
  action: ScheduleAction;
}

export type BackendKind = 'systemd' | 'cron' | 'schtasks' | 'launchd' | 'none';

export interface Backend {
  kind: BackendKind;
  available: boolean;
  detail: string;
}

export interface RemoveResult {
  removed: boolean;
  warnings: string[];
}
