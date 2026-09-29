import { describe, expect, it } from 'vitest';
import type { DupGroup } from '../api/duplicates';
import {
  blockedGroups,
  canDelete,
  canSelect,
  fromAuto,
  groupState,
  groupsTouched,
  keepOnly,
  removeFiles,
  selectedBytes,
  selectedPaths,
  toggleFile,
} from './dupSelection';

const file = (path: string, bytes = 10, modified = 1) => ({ path, bytes, modified });
const group = (id: string, paths: string[], bytes = 10): DupGroup => ({
  groupId: id,
  key: { bytes },
  files: paths.map((p) => file(p, bytes)),
  wastedBytes: bytes * (paths.length - 1),
});

const G0 = group('g0', ['/a/1', '/b/1', '/c/1']);
const G1 = group('g1', ['/a/2', '/b/2'], 100);
const GROUPS = [G0, G1];

describe('duplicate selection rules', () => {
  it('ticking never selects the last file of a group', () => {
    let sel = new Set<string>();
    sel = toggleFile(G0, sel, '/a/1');
    sel = toggleFile(G0, sel, '/b/1');
    expect([...sel].sort()).toEqual(['/a/1', '/b/1']);
    expect(canSelect(G0, sel, '/c/1')).toBe(false);
    // the click is refused: the very same contents come back
    const again = toggleFile(G0, sel, '/c/1');
    expect([...again].sort()).toEqual(['/a/1', '/b/1']);
    expect(blockedGroups(GROUPS, again)).toEqual([]);
  });

  it('unticking is always allowed and frees the lock', () => {
    const sel = new Set(['/a/1', '/b/1']);
    const next = toggleFile(G0, sel, '/a/1');
    expect(next.has('/a/1')).toBe(false);
    expect(canSelect(G0, next, '/c/1')).toBe(true);
    expect(canSelect(G0, sel, '/a/1')).toBe(true);
  });

  it('a two-file group allows exactly one tick', () => {
    let sel = toggleFile(G1, new Set(), '/a/2');
    expect(canSelect(G1, sel, '/b/2')).toBe(false);
    sel = toggleFile(G1, sel, '/b/2');
    expect([...sel]).toEqual(['/a/2']);
  });

  it('groups are independent', () => {
    const sel = toggleFile(G1, toggleFile(G0, new Set(), '/a/1'), '/a/2');
    expect(groupState(G0, sel)).toEqual({ selected: 1, total: 3, atLimit: false, all: false });
    expect(groupState(G1, sel)).toEqual({ selected: 1, total: 2, atLimit: true, all: false });
  });

  it('blockedGroups and canDelete catch a fully selected group from any source', () => {
    const bad = new Set(['/a/2', '/b/2', '/a/1']);
    expect(blockedGroups(GROUPS, bad)).toEqual(['g1']);
    expect(canDelete(GROUPS, bad)).toBe(false);
    expect(canDelete(GROUPS, new Set())).toBe(false);
    expect(canDelete(GROUPS, new Set(['/a/1']))).toBe(true);
  });

  it('keepOnly selects every other copy and unselects the kept one', () => {
    const sel = keepOnly(G0, new Set(['/b/1']), '/b/1');
    expect([...sel].sort()).toEqual(['/a/1', '/c/1']);
    expect(blockedGroups([G0], sel)).toEqual([]);
  });

  it('fromAuto ignores unknown groups/paths and refuses a rule that selects all', () => {
    const sel = fromAuto(GROUPS, {
      count: 0,
      bytes: 0,
      groups: [
        { groupId: 'g0', selected: ['/a/1', '/b/1', '/gone'] },
        { groupId: 'g1', selected: ['/a/2', '/b/2'] }, // would select all: dropped
        { groupId: 'g9', selected: ['/x'] },
      ],
    });
    expect([...sel].sort()).toEqual(['/a/1', '/b/1']);
  });

  it('counts and sums only what is listed', () => {
    const sel = new Set(['/a/1', '/b/1', '/a/2', '/not-listed']);
    expect(selectedPaths(GROUPS, sel)).toEqual(['/a/1', '/b/1', '/a/2']);
    expect(selectedBytes(GROUPS, sel)).toBe(10 + 10 + 100);
    expect(groupsTouched(GROUPS, sel)).toBe(2);
  });

  it('removeFiles drops groups that are no longer duplicates and recomputes waste', () => {
    const after = removeFiles(GROUPS, new Set(['/a/1', '/a/2']));
    expect(after.map((g) => g.groupId)).toEqual(['g0']);
    expect(after[0]?.files.map((f) => f.path)).toEqual(['/b/1', '/c/1']);
    expect(after[0]?.wastedBytes).toBe(10);
  });
});
