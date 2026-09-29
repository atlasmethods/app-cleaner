/** Planned methods for the `disk_analyzer` feature. Request/response types are added when it is implemented. */
export const DISK_ANALYZER_METHODS = [
  'disk_analyzer.scan',
  'disk_analyzer.top_files',
  'disk_analyzer.delete',
] as const;

export type DiskAnalyzerMethod = (typeof DISK_ANALYZER_METHODS)[number];
