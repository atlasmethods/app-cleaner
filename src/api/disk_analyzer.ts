/** Mirrors `sweep_core::features::disk_analyzer` (serde camelCase). */
export const DISK_ANALYZER_METHODS = [
  'disk_analyzer.list_drives',
  'disk_analyzer.scan',
  'disk_analyzer.files',
  'disk_analyzer.tree',
  'disk_analyzer.delete',
  'disk_analyzer.open_folder',
] as const;

export type DiskAnalyzerMethod = (typeof DISK_ANALYZER_METHODS)[number];

export type FileCategory = 'pictures' | 'music' | 'documents' | 'video' | 'compressed' | 'email' | 'other';

export const FILE_CATEGORIES: readonly FileCategory[] = [
  'pictures',
  'music',
  'documents',
  'video',
  'compressed',
  'email',
  'other',
];

export interface DriveInfo {
  name: string;
  mount: string;
  fs: string;
  total: number;
  available: number;
  removable: boolean;
}

export interface ScanParams {
  paths: string[];
  categories?: FileCategory[];
}

export type CategoryTotals = Record<FileCategory, { files: number; bytes: number }>;

export interface ScanSummary {
  scanId: string;
  roots: string[];
  totals: CategoryTotals;
  totalFiles: number;
  totalBytes: number;
  stored: number;
  truncated: boolean;
  errors: { count: number; samples: string[] };
  durationMs: number;
}

export type FileSort = 'size' | 'name' | 'modified';

export interface FilesParams {
  scanId: string;
  category?: FileCategory;
  folder?: string;
  sort?: FileSort;
  offset?: number;
  limit?: number;
}

export interface FileRow {
  path: string;
  name: string;
  bytes: number;
  /** Unix seconds. */
  modified: number;
  category: FileCategory;
}

export interface FilesPage {
  total: number;
  totalBytes: number;
  offset: number;
  limit: number;
  truncated: boolean;
  files: FileRow[];
}

export interface TreeChild {
  path: string;
  name: string;
  bytes: number;
  files: number;
}

export interface TreeNode {
  /** `null` for the virtual top level of a multi-folder scan. */
  path: string | null;
  name: string;
  bytes: number;
  files: number;
  ownBytes: number;
  ownFiles: number;
  canGoUp: boolean;
  /** Folder to go up to; `null` means the top level (call `tree` without a path). */
  parent: string | null;
  childCount: number;
  children: TreeChild[];
}

export interface DeleteFilesResult {
  results: { path: string; ok: boolean; error?: string; bytes: number }[];
  deleted: number;
  freedBytes: number;
  totals: CategoryTotals;
  totalFiles: number;
  totalBytes: number;
}
