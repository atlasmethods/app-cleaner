import { describe, expect, it } from 'vitest';
import type { CategoryInfo, FixResult, Issue } from '../api/registry_cleaner';
import {
  confirmMessage,
  defaultCategories,
  failures,
  fixSummary,
  groupIssues,
  groupState,
  plural,
  preselectIssues,
  pruneSelection,
  setMany,
  toggleOne,
} from './registry';

const cat = (id: string, defaultSelected = true): CategoryInfo => ({
  id,
  label: id.toUpperCase(),
  description: '',
  severity: 'low',
  defaultSelected,
});
const issue = (id: string, category: string, extra: Partial<Issue> = {}): Issue => ({
  id,
  category,
  description: `d-${id}`,
  location: `loc-${id}`,
  severity: 'low',
  needsAdmin: false,
  ...extra,
});

const CATS = [cat('a'), cat('b', false), cat('c')];
const ISSUES = [issue('1', 'c'), issue('2', 'a'), issue('3', 'b'), issue('4', 'a'), issue('5', 'zzz')];

describe('grouping', () => {
  it('follows the category order, drops empty categories and appends unknown ones', () => {
    const g = groupIssues(ISSUES, CATS);
    expect(g.map((x) => x.category.id)).toEqual(['a', 'b', 'c', 'zzz']);
    expect(g[0]!.issues.map((i) => i.id)).toEqual(['2', '4']);
    expect(groupIssues([], CATS)).toEqual([]);
  });
});

describe('selection', () => {
  it('defaults follow the categories', () => {
    expect([...defaultCategories(CATS)]).toEqual(['a', 'c']);
    expect([...preselectIssues(ISSUES, CATS)].sort()).toEqual(['1', '2', '4']);
  });

  it('group state, select many and toggle', () => {
    const a = ISSUES.filter((i) => i.category === 'a');
    let sel = new Set<string>();
    expect(groupState(a, sel)).toBe('none');
    sel = toggleOne(sel, '2');
    expect(groupState(a, sel)).toBe('some');
    sel = setMany(sel, a, true);
    expect(groupState(a, sel)).toBe('all');
    sel = setMany(sel, a, false);
    expect(sel.size).toBe(0);
    expect(toggleOne(new Set(['x']), 'x').size).toBe(0);
  });

  it('prunes ids that no longer exist', () => {
    expect([...pruneSelection(new Set(['1', 'gone']), ISSUES)]).toEqual(['1']);
  });

  it('does not mutate its input', () => {
    const s = new Set(['1']);
    setMany(s, ISSUES, true);
    toggleOne(s, '2');
    expect([...s]).toEqual(['1']);
  });
});

describe('texts', () => {
  it('plural', () => {
    expect(plural(1, 'item')).toBe('1 item');
    expect(plural(2, 'item')).toBe('2 items');
    expect(plural(2, 'registry entry', 'registry entries')).toBe('2 registry entries');
  });

  it('confirmation states the backup and the admin prompt', () => {
    const m = confirmMessage(3, 2, true);
    expect(m).toContain('3 registry entries');
    expect(m).toContain('backup');
    expect(m).toContain('2 of them need administrator rights');
    expect(confirmMessage(1, 0, false)).toContain('1 item will be fixed');
    expect(confirmMessage(1, 0, false)).not.toContain('administrator');
  });

  it('fix summary and failures', () => {
    const r: FixResult = {
      backupId: 'config-1',
      fixed: 2,
      failed: 1,
      results: [
        { id: '1', ok: true },
        { id: '2', ok: false, error: 'denied' },
        { id: '9', ok: false },
      ],
    };
    expect(fixSummary(r)).toBe('Fixed 2, 1 could not be fixed.');
    expect(fixSummary({ ...r, failed: 0 })).toBe('Fixed 2.');
    expect(fixSummary({ backupId: null, fixed: 0, failed: 0, results: [] })).toBe('Nothing was changed.');
    expect(failures(r, ISSUES)).toEqual([
      { id: '2', description: 'd-2', error: 'denied' },
      { id: '9', description: '9', error: 'Unknown error' },
    ]);
  });
});
