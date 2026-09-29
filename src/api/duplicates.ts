/** Mirrors `sweep_core::features::duplicates` (serde camelCase). */
export const DUPLICATES_METHODS = [
  'duplicates.scan',
  'duplicates.groups',
  'duplicates.auto_select',
  'duplicates.delete',
  'duplicates.export',
] as const;

export type DuplicatesMethod = (typeof DUPLICATES_METHODS)[number];

export interface MatchBy {
  name: boolean;
  size: boolean;
  modified: boolean;
  content: boolean;
}

export interface DupScanParams {
  paths: string[];
  excludePaths?: string[];
  matchBy: MatchBy;
  minSize?: number;
  maxSize?: number;
  includeHidden?: boolean;
  includeSystem?: boolean;
  skipZeroByte?: boolean;
  followLinks?: boolean;
}

export interface DupFile {
  path: string;
  bytes: number;
  /** Unix seconds. */
  modified: number;
}

export interface DupGroup {
  groupId: string;
  key: { name?: string; bytes?: number; modified?: number; hash?: string };
  files: DupFile[];
  wastedBytes: number;
}

export interface DupScanResult {
  scanId: string;
  groups: DupGroup[];
  totalGroups: number;
  totalFiles: number;
  wastedBytes: number;
  scannedFiles: number;
  truncatedGroups: boolean;
  errors: { count: number; samples: string[] };
  durationMs: number;
}

export interface DupGroupsPage {
  groups: DupGroup[];
  offset: number;
  totalGroups: number;
  totalFiles: number;
  wastedBytes: number;
}

export type AutoRule = 'keep_newest' | 'keep_oldest' | 'keep_shortest_path' | 'keep_in_folder';

export interface AutoSelectResult {
  groups: { groupId: string; selected: string[] }[];
  count: number;
  bytes: number;
}

export interface DupDeleteResult {
  results: { path: string; ok: boolean; error?: string; bytes: number }[];
  deleted: number;
  freedBytes: number;
  remainingGroups: number;
  remainingFiles: number;
  wastedBytes: number;
}

export interface ExportResult {
  format: 'csv' | 'txt';
  filename: string;
  text: string;
}
