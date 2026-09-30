/** Mirrors `sweep_core::features::smart_cleaning` (serde camelCase). */
import type { SmartSettings } from './settings';

export const SMART_CLEANING_METHODS = [
  'smart_cleaning.get_config',
  'smart_cleaning.set_config',
  'smart_cleaning.check',
  'smart_cleaning.status',
] as const;

export type SmartCleaningMethod = (typeof SMART_CLEANING_METHODS)[number];

export type SmartConfig = Required<SmartSettings>;

export interface CheckResult {
  junkBytes: number;
  thresholdBytes: number;
  overThreshold: boolean;
  cleaned?: { removedBytes: number; removedFiles: number; removedRows: number; historyId?: string };
  cleanError?: string;
}

export interface AgentStatus {
  running: boolean;
  pid?: number;
  /** RFC 3339 */
  since?: string;
  lastCheck?: string;
  lastJunkBytes?: number;
  lastAction?: string;
  lastActionAt?: string;
}
