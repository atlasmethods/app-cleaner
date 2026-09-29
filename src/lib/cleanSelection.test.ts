import { describe, expect, it } from 'vitest';
import type { AnalyzeItem, GroupInfo, RuleInfo, RulesListing } from '../api/cleaner';
import {
  applyPlan,
  groupState,
  hasContent,
  orderedSelection,
  planCategoryToggle,
  planGroupToggle,
  planRuleToggle,
  selectionOf,
  slug,
  sortResults,
  visibleResults,
} from './cleanSelection';

const rule = (id: string, enabled: boolean, warning?: string): RuleInfo => ({
  id,
  name: id,
  description: '',
  enabled,
  defaultEnabled: enabled,
  ...(warning ? { warning } : {}),
});

const chrome: GroupInfo = {
  group: 'Google Chrome',
  rules: [rule('chrome.cache', true), rule('chrome.history', true), rule('chrome.passwords', false, 'Deletes passwords')],
};
const temp: GroupInfo = { group: 'System', rules: [rule('linux.temp', false), rule('linux.trash', false)] };
const listing: RulesListing = {
  categories: [
    { category: 'browser', label: 'Browsers', groups: [chrome] },
    { category: 'system', label: 'System', groups: [temp] },
  ],
};

describe('selection state', () => {
  it('reads the enabled rules from the listing', () => {
    expect([...selectionOf(listing)]).toEqual(['chrome.cache', 'chrome.history']);
  });

  it('computes tri-state group state', () => {
    expect(groupState(chrome, selectionOf(listing))).toBe('some');
    expect(groupState(temp, selectionOf(listing))).toBe('none');
    expect(groupState(temp, new Set(['linux.temp', 'linux.trash']))).toBe('all');
    expect(groupState({ group: 'x', rules: [] }, new Set())).toBe('none');
  });
});

describe('toggle plans', () => {
  it('a group with some selected turns everything on and reports warnings', () => {
    const plan = planGroupToggle(chrome, selectionOf(listing));
    expect(plan.on).toBe(true);
    expect(plan.ids).toEqual(['chrome.passwords']);
    expect(plan.warnings.map((w) => w.id)).toEqual(['chrome.passwords']);
  });

  it('a fully selected group turns everything off, without warnings', () => {
    const all = new Set(chrome.rules.map((r) => r.id));
    const plan = planGroupToggle(chrome, all);
    expect(plan.on).toBe(false);
    expect(plan.ids).toEqual(['chrome.cache', 'chrome.history', 'chrome.passwords']);
    expect(plan.warnings).toEqual([]);
    expect(applyPlan(all, plan).size).toBe(0);
  });

  it('an empty group turns on; no warnings when none of its rules has one', () => {
    const plan = planGroupToggle(temp, new Set());
    expect(plan).toMatchObject({ on: true, ids: ['linux.temp', 'linux.trash'], warnings: [] });
  });

  it('a rule toggle warns only when turning a warned rule on', () => {
    const pw = chrome.rules[2]!;
    expect(planRuleToggle(pw, new Set()).warnings).toHaveLength(1);
    expect(planRuleToggle(pw, new Set(['chrome.passwords']))).toMatchObject({ on: false, warnings: [] });
    expect(planRuleToggle(chrome.rules[0]!, new Set()).warnings).toEqual([]);
  });

  it('category toggle mirrors group toggle', () => {
    const cat = listing.categories[0]!;
    expect(planCategoryToggle(cat, selectionOf(listing))).toMatchObject({ on: true, ids: ['chrome.passwords'] });
    const all = new Set(chrome.rules.map((r) => r.id));
    expect(planCategoryToggle(cat, all)).toMatchObject({ on: false });
  });

  it('applyPlan does not mutate its input', () => {
    const before = new Set(['a']);
    const next = applyPlan(before, { on: true, ids: ['b'], warnings: [] });
    expect([...before]).toEqual(['a']);
    expect([...next].sort()).toEqual(['a', 'b']);
  });

  it('persists in listing order regardless of click order', () => {
    const sel = new Set(['linux.trash', 'chrome.passwords', 'chrome.cache']);
    expect(orderedSelection(listing, sel)).toEqual(['chrome.cache', 'chrome.passwords', 'linux.trash']);
    expect(orderedSelection(listing, new Set(['unknown.rule']))).toEqual([]);
  });
});

const item = (over: Partial<AnalyzeItem>): AnalyzeItem => ({
  ruleId: 'a.b',
  name: 'B',
  group: 'A',
  category: 'browser',
  files: 0,
  bytes: 0,
  rows: 0,
  samplePaths: [],
  appRunning: false,
  errors: [],
  actions: [],
  ...over,
});

describe('results', () => {
  it('sorts by size, then files, rows and name', () => {
    const sorted = sortResults([
      item({ ruleId: 'small', bytes: 10, files: 1 }),
      item({ ruleId: 'big', bytes: 1000, files: 1 }),
      item({ ruleId: 'more-files', bytes: 10, files: 5 }),
      item({ ruleId: 'rows', bytes: 0, rows: 9 }),
      item({ ruleId: 'zzz', bytes: 0, rows: 1, group: 'Z' }),
      item({ ruleId: 'aaa', bytes: 0, rows: 1, group: 'A' }),
    ]);
    expect(sorted.map((i) => i.ruleId)).toEqual(['big', 'more-files', 'small', 'rows', 'aaa', 'zzz']);
  });

  it('hides empty items but keeps ones with actions or errors', () => {
    const items = [
      item({ ruleId: 'empty' }),
      item({ ruleId: 'action', actions: ['Clear clipboard'] }),
      item({ ruleId: 'err', errors: [{ path: '/x', message: 'boom' }] }),
      item({ ruleId: 'data', bytes: 5, files: 1 }),
    ];
    expect(visibleResults(items).map((i) => i.ruleId).sort()).toEqual(['action', 'data', 'err']);
    expect(hasContent(items[0]!)).toBe(false);
    expect(hasContent(items[1]!)).toBe(true);
  });

  it('slugs group names for test ids', () => {
    expect(slug('Google Chrome')).toBe('google-chrome');
    expect(slug('JetBrains IDEs')).toBe('jetbrains-ides');
    expect(slug('  C++ / Tools ')).toBe('c-tools');
  });
});
