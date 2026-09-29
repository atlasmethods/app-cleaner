/** Mirrors `sweep_core::features::optimizer` (serde camelCase). */
export const OPTIMIZER_METHODS = [
  'optimizer.analyze',
  'optimizer.sleep',
  'optimizer.wake',
  'optimizer.enforce',
] as const;

export type OptimizerMethod = (typeof OPTIMIZER_METHODS)[number];

export interface OptimizerProcess {
  pid: number;
  name: string;
  memoryBytes: number;
  cpuPercent: number;
}

export interface OptimizerApp {
  appId: string;
  name: string;
  icon?: string | null;
  processes: OptimizerProcess[];
  startupIds: string[];
  serviceIds: string[];
  backgroundMemoryBytes: number;
  cpuPercent: number;
  sleeping: boolean;
  protected: boolean;
}

export interface OptimizerTotals {
  apps: number;
  runningApps: number;
  backgroundMemoryBytes: number;
  sleepingApps: number;
  startupItems: number;
}

export interface OptimizerAnalysis {
  apps: OptimizerApp[];
  totals: OptimizerTotals;
  /** The saved sleep state could not be read (it is never reset silently). */
  stateError?: string | null;
}

export interface SleepResult {
  appId: string;
  name: string;
  ok: boolean;
  error?: string;
  disabledItems?: string[];
  stoppedProcesses?: number;
  stillRunning?: number;
  note?: string | null;
  errors?: string[];
}

export interface WakeResult {
  appId: string;
  name: string;
  ok: boolean;
  error?: string;
  restoredItems?: string[];
  missingItems?: string[];
  errors?: string[];
}

export interface AppResults<T> {
  results: T[];
}
