import type { CleanReport, RuleClean } from '../api/cleaner';

/**
 * Combine a first clean with a retry of some rules: results of the retry replace the
 * earlier (skipped) results of the same rules. Totals are recomputed from the results.
 */
export function mergeReports(prev: CleanReport, retry: CleanReport): CleanReport {
  const retried = new Map<string, RuleClean>(retry.results.map((r) => [r.ruleId, r]));
  const results = prev.results.map((r) => retried.get(r.ruleId) ?? r);
  for (const r of retry.results) {
    if (!prev.results.some((p) => p.ruleId === r.ruleId)) results.push(r);
  }
  return {
    ...prev,
    results,
    totalFiles: results.reduce((n, r) => n + r.removedFiles, 0),
    totalBytes: results.reduce((n, r) => n + r.removedBytes, 0),
    totalRows: results.reduce((n, r) => n + r.removedRows, 0),
    durationMs: prev.durationMs + retry.durationMs,
    cancelled: prev.cancelled || retry.cancelled,
    historyId: retry.historyId ?? prev.historyId,
  };
}

/** Distinct app names (rule groups) that blocked a clean. */
export function blockedApps(report: CleanReport): string[] {
  const names = new Set<string>();
  for (const r of report.results) {
    if (r.skipped === 'app_running') r.runningApps.forEach((a) => names.add(a));
  }
  return [...names];
}

export function blockedRuleIds(report: CleanReport): string[] {
  return report.results.filter((r) => r.skipped === 'app_running').map((r) => r.ruleId);
}
