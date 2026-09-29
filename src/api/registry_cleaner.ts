/** Mirrors `sweep_core::features::registry_cleaner` (serde camelCase). */
export const REGISTRY_CLEANER_METHODS = [
  'registry_cleaner.categories',
  'registry_cleaner.scan',
  'registry_cleaner.fix',
  'registry_cleaner.list_backups',
  'registry_cleaner.restore_backup',
  'registry_cleaner.delete_backup',
] as const;

export type RegistryCleanerMethod = (typeof REGISTRY_CLEANER_METHODS)[number];

export type Platform = 'windows' | 'linux' | 'macos';
export type Severity = 'low' | 'medium';

export interface CategoryInfo {
  id: string;
  label: string;
  description: string;
  severity: Severity;
  /** Pre-ticked in the UI. */
  defaultSelected: boolean;
}

export interface CategoriesResult {
  platform: Platform;
  /** "Registry" on Windows, "Config Issues" elsewhere. */
  title: string;
  categories: CategoryInfo[];
}

export interface Issue {
  id: string;
  category: string;
  description: string;
  /** Registry key path or file path. */
  location: string;
  value?: string;
  data?: string;
  severity: Severity;
  /** Fixing needs administrator rights. */
  needsAdmin: boolean;
}

export interface ScanParams {
  categories?: string[];
}

export interface ScanResult {
  platform: Platform;
  title: string;
  issues: Issue[];
  counts: Record<string, number>;
  scanned: string[];
  skipped: { category: string; reason: string }[];
}

export interface FixParams {
  issueIds: string[];
  backup: true;
  categories?: string[];
}

export interface IssueResult {
  id: string;
  ok: boolean;
  error?: string;
}

export interface FixResult {
  backupId: string | null;
  fixed: number;
  failed: number;
  results: IssueResult[];
}

export interface BackupInfo {
  id: string;
  createdAt: string;
  issueCount: number;
  sizeBytes: number;
  platform: Platform;
  kind: 'registry' | 'config';
}

export interface RestoreItem {
  target: string;
  ok: boolean;
  error?: string;
}

export interface RestoreOutcome {
  id: string;
  ok: boolean;
  restored: number;
  failed: number;
  results: RestoreItem[];
}
