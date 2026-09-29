import { describe, expect, it } from 'vitest';
import type { CleanReport, RuleClean } from '../api/cleaner';
import { blockedApps, blockedRuleIds, mergeReports } from './mergeReports';

const r = (ruleId: string, over: Partial<RuleClean> = {}): RuleClean => ({
  ruleId,
  removedFiles: 0,
  removedBytes: 0,
  removedRows: 0,
  failed: [],
  actions: [],
  runningApps: [],
  closedApps: [],
  ...over,
});

const rep = (results: RuleClean[]): CleanReport => ({
  results,
  totalFiles: results.reduce((n, x) => n + x.removedFiles, 0),
  totalBytes: results.reduce((n, x) => n + x.removedBytes, 0),
  totalRows: results.reduce((n, x) => n + x.removedRows, 0),
  durationMs: 10,
  cancelled: false,
});

describe('mergeReports', () => {
  it('replaces skipped results by the retry and recomputes totals', () => {
    const first = rep([
      r('a.cache', { removedFiles: 2, removedBytes: 200 }),
      r('chrome.cache', { skipped: 'app_running', runningApps: ['Google Chrome'] }),
      r('chrome.history', { skipped: 'app_running', runningApps: ['Google Chrome'] }),
    ]);
    expect(blockedApps(first)).toEqual(['Google Chrome']);
    expect(blockedRuleIds(first)).toEqual(['chrome.cache', 'chrome.history']);
    const retry = rep([
      r('chrome.cache', { removedFiles: 5, removedBytes: 1000, closedApps: ['Google Chrome'] }),
      r('chrome.history', { removedRows: 7 }),
    ]);
    const merged = mergeReports(first, { ...retry, historyId: 'h2' });
    expect(merged.results.map((x) => x.ruleId)).toEqual(['a.cache', 'chrome.cache', 'chrome.history']);
    expect(merged.results.every((x) => !x.skipped)).toBe(true);
    expect([merged.totalFiles, merged.totalBytes, merged.totalRows]).toEqual([7, 1200, 7]);
    expect(merged.durationMs).toBe(20);
    expect(merged.historyId).toBe('h2');
    expect(blockedApps(merged)).toEqual([]);
  });

  it('keeps a rule that is still blocked after the retry', () => {
    const first = rep([r('chrome.cache', { skipped: 'app_running', runningApps: ['Google Chrome'] })]);
    const retry = rep([r('chrome.cache', { skipped: 'app_running', runningApps: ['Google Chrome'] })]);
    expect(blockedRuleIds(mergeReports(first, retry))).toEqual(['chrome.cache']);
  });

  it('dedupes app names', () => {
    const first = rep([
      r('a.x', { skipped: 'app_running', runningApps: ['Firefox'] }),
      r('a.y', { skipped: 'app_running', runningApps: ['Firefox'] }),
      r('a.z', { skipped: 'in_use' }),
    ]);
    expect(blockedApps(first)).toEqual(['Firefox']);
  });
});
