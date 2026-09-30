/** Mirrors `sweep_core::features::health` (serde camelCase). */
import type { CloseBrowsers } from './settings';
import type { StartupImpact } from './startup';

export const HEALTH_METHODS = ['health.analyze', 'health.last', 'health.fix'] as const;

export type HealthMethod = (typeof HEALTH_METHODS)[number];

export type HealthStatus = 'good' | 'warning' | 'problem' | 'unavailable';
export type CategoryId = 'privacy' | 'space' | 'speed' | 'security';

export interface StartupFinding {
  id: string;
  name: string;
  impact: StartupImpact;
}

export interface AppFinding {
  appId: string;
  name: string;
  memoryBytes: number;
}

export interface UpdateFinding {
  id: string;
  name: string;
  currentVersion: string;
  newVersion: string;
  security: boolean;
}

export type Finding =
  | { kind: 'trackers'; count: number; browsers: string[] }
  | { kind: 'history'; count: number; browsers: string[] }
  | { kind: 'junk'; group: string; bytes: number; files: number; rows: number }
  | { kind: 'startup'; items: StartupFinding[] }
  | { kind: 'background_apps'; apps: AppFinding[] }
  | { kind: 'updates'; count: number; security: number; items: UpdateFinding[] };

export interface HealthCategory {
  id: CategoryId;
  title: string;
  status: HealthStatus;
  /** For `unavailable`: the reason. */
  summary: string;
  findings: Finding[];
  /** There is something `health.fix` can do for this category. */
  fixable: boolean;
  metrics: Record<string, number>;
}

export interface HealthReport {
  /** 0..100, `null` when no category could be measured. */
  score: number | null;
  /** RFC 3339 */
  scannedAt: string;
  categories: HealthCategory[];
}

export type FixPart = 'privacy' | 'space' | 'startup' | 'sleep' | 'updates';
export type PartStatus = 'done' | 'partial' | 'failed' | 'not_run';

export interface ItemResult {
  id: string;
  name: string;
  ok: boolean;
  message: string;
}

export interface PartResult {
  part: FixPart;
  status: PartStatus;
  message: string;
  removedBytes: number;
  removedFiles: number;
  removedRows: number;
  /** Privacy / space: browsers that were running, so their data was left alone. */
  blockedApps: string[];
  items: ItemResult[];
}

export interface FixReport {
  parts: PartResult[];
  cancelled: boolean;
  /** Fresh analysis after the fix; `null` when cancelled. */
  report: HealthReport | null;
}

export interface FixParams {
  privacy?: boolean;
  space?: boolean;
  startupIds?: string[];
  sleepAppIds?: string[];
  updateIds?: string[];
  closeApps?: CloseBrowsers;
}
