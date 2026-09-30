/** Mirrors `sweep_core::features::cleaner` (serde camelCase). */
import type { CloseBrowsers } from './settings';

export const CLEANER_METHODS = [
  'cleaner.list_rules',
  'cleaner.analyze',
  'cleaner.clean',
  'cleaner.history',
] as const;

export type CleanerMethod = (typeof CLEANER_METHODS)[number];

export type Category = 'browser' | 'system' | 'application';

export interface RuleInfo {
  id: string;
  name: string;
  description: string;
  warning?: string;
  /** Currently selected (settings.selectedRules, or the rule's own default). */
  enabled: boolean;
  defaultEnabled: boolean;
}

export interface GroupInfo {
  group: string;
  rules: RuleInfo[];
}

export interface CategoryInfo {
  category: Category;
  label: string;
  groups: GroupInfo[];
}

export interface RulesListing {
  categories: CategoryInfo[];
}

export interface PathErr {
  path: string;
  message: string;
}

export interface AnalyzeItem {
  ruleId: string;
  name: string;
  group: string;
  category: Category;
  files: number;
  bytes: number;
  rows: number;
  samplePaths: string[];
  appRunning: boolean;
  errors: PathErr[];
  actions: string[];
  /** Temp folders / files left alone because a program still uses them (not an error). */
  inUseSkipped?: number;
}

export interface AnalyzeReport {
  items: AnalyzeItem[];
  totalFiles: number;
  totalBytes: number;
  totalRows: number;
  durationMs: number;
}

export type Skipped = 'app_running' | 'in_use' | 'unsupported';

export interface RuleClean {
  ruleId: string;
  removedFiles: number;
  removedBytes: number;
  removedRows: number;
  failed: PathErr[];
  skipped?: Skipped;
  actions: string[];
  runningApps: string[];
  closedApps: string[];
  /** Temp folders / files left alone because a program still uses them (not an error). */
  inUseSkipped?: number;
}

export interface CleanReport {
  results: RuleClean[];
  totalFiles: number;
  totalBytes: number;
  totalRows: number;
  durationMs: number;
  cancelled: boolean;
  historyId?: string;
}

export interface CleanParams {
  ruleIds?: string[];
  closeApps?: CloseBrowsers;
}

export type HistorySource = 'manual' | 'auto' | 'smart' | 'scheduled' | 'health';

export interface HistoryEntry {
  id: string;
  /** RFC 3339 */
  at: string;
  totalBytes: number;
  totalFiles: number;
  totalRows: number;
  ruleIds: string[];
  source: HistorySource;
}
