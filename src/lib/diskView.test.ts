import { describe, expect, it } from 'vitest';
import type { CategoryTotals } from '../api/disk_analyzer';
import { breadcrumbs, categoryBars, folderBars, percentOf, splitPath } from './diskView';

const totals = (over: Partial<Record<keyof CategoryTotals, [number, number]>>): CategoryTotals => {
  const base = Object.fromEntries(
    ['pictures', 'music', 'documents', 'video', 'compressed', 'email', 'other'].map((c) => [c, { files: 0, bytes: 0 }]),
  ) as CategoryTotals;
  for (const [k, v] of Object.entries(over)) base[k as keyof CategoryTotals] = { files: v[0], bytes: v[1] };
  return base;
};

describe('categoryBars', () => {
  it('sorts by size, hides empty categories and reports percentages of the total', () => {
    const bars = categoryBars(totals({ pictures: [2, 300], video: [1, 5000], other: [2, 10], music: [1, 1000] }));
    expect(bars.map((b) => b.category)).toEqual(['video', 'music', 'pictures', 'other']);
    expect(bars[0]?.percent).toBe(79.2);
    expect(bars.reduce((s, b) => s + b.percent, 0)).toBeGreaterThan(99.5);
    expect(bars.reduce((s, b) => s + b.percent, 0)).toBeLessThan(100.5);
  });

  it('is empty without files and tolerates zero-byte files', () => {
    expect(categoryBars(totals({}))).toEqual([]);
    const bars = categoryBars(totals({ other: [3, 0] }));
    expect(bars).toHaveLength(1);
    expect(bars[0]?.percent).toBe(0);
  });

  it('ties keep the fixed category order', () => {
    const bars = categoryBars(totals({ email: [1, 10], pictures: [1, 10], music: [1, 10] }));
    expect(bars.map((b) => b.category)).toEqual(['pictures', 'music', 'email']);
  });
});

describe('percentOf', () => {
  it('clamps and rounds to one decimal', () => {
    expect(percentOf(1, 3)).toBe(33.3);
    expect(percentOf(5, 0)).toBe(0);
    expect(percentOf(9, 3)).toBe(100);
    expect(percentOf(-1, 3)).toBe(0);
  });
});

describe('folderBars', () => {
  it('scales to the biggest sibling but keeps tiny folders visible', () => {
    const bars = folderBars(
      [
        { path: '/r/a', name: 'a', bytes: 900, files: 1 },
        { path: '/r/b', name: 'b', bytes: 1, files: 1 },
        { path: '/r/c', name: 'c', bytes: 0, files: 0 },
      ],
      1000,
    );
    expect(bars.map((b) => b.width)).toEqual([100, 2, 0]);
    expect(bars.map((b) => b.percent)).toEqual([90, 0.1, 0]);
  });
});

describe('breadcrumbs', () => {
  it('builds the trail below the root', () => {
    const c = breadcrumbs('/home/u/data/sub/deep', ['/home/u/data']);
    expect(c.map((x) => x.label)).toEqual(['/home/u/data', 'sub', 'deep']);
    expect(c.map((x) => x.path)).toEqual(['/home/u/data', '/home/u/data/sub', '/home/u/data/sub/deep']);
  });

  it('adds an "All" crumb for several roots and for the virtual top level', () => {
    const roots = ['/a', '/b'];
    expect(breadcrumbs(null, roots)).toEqual([{ label: 'All', path: null }]);
    const c = breadcrumbs('/b/x', roots);
    expect(c.map((x) => x.label)).toEqual(['All', '/b', 'x']);
  });

  it('picks the deepest matching root and does not confuse prefixes', () => {
    const c = breadcrumbs('/data2/x', ['/data', '/data2']);
    expect(c.map((x) => x.label)).toEqual(['All', '/data2', 'x']);
  });

  it('handles Windows separators', () => {
    const c = breadcrumbs('C:\\Users\\me\\Pictures', ['C:\\Users']);
    expect(c.map((x) => x.label)).toEqual(['C:\\Users', 'me', 'Pictures']);
    expect(c[2]?.path).toBe('C:\\Users\\me\\Pictures');
  });
});

describe('splitPath', () => {
  it('splits both separators', () => {
    expect(splitPath('/a/b/c.txt')).toEqual({ dir: '/a/b', name: 'c.txt' });
    expect(splitPath('C:\\a\\c.txt')).toEqual({ dir: 'C:\\a', name: 'c.txt' });
    expect(splitPath('c.txt')).toEqual({ dir: '', name: 'c.txt' });
    expect(splitPath('/c.txt')).toEqual({ dir: '/', name: 'c.txt' });
  });
});
