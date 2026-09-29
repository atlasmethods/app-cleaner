import type { AutoSelectResult, DupGroup } from '../api/duplicates';

/**
 * Selection rules of the duplicate finder. The invariant everywhere: a group never has
 * every file selected, so "Delete selected" can never remove the last copy. (The server
 * enforces the same rule; this keeps the UI from ever offering it.)
 */
export type Selection = ReadonlySet<string>;

function selectedIn(group: DupGroup, sel: Selection): number {
  let n = 0;
  for (const f of group.files) if (sel.has(f.path)) n++;
  return n;
}

export interface GroupState {
  selected: number;
  total: number;
  /** Every file but one is selected: the remaining one is locked. */
  atLimit: boolean;
  /** All files selected (never allowed to be deleted). */
  all: boolean;
}

export function groupState(group: DupGroup, sel: Selection): GroupState {
  const selected = selectedIn(group, sel);
  const total = group.files.length;
  return { selected, total, atLimit: selected === total - 1, all: selected >= total };
}

/** Can `path` be ticked right now? Unticking is always allowed. */
export function canSelect(group: DupGroup, sel: Selection, path: string): boolean {
  if (sel.has(path)) return true;
  return selectedIn(group, sel) < group.files.length - 1;
}

/** Toggle one file; ticking the last unticked file of a group is refused (same set back). */
export function toggleFile(group: DupGroup, sel: Selection, path: string): Set<string> {
  const next = new Set(sel);
  if (next.has(path)) {
    next.delete(path);
  } else if (canSelect(group, sel, path)) {
    next.add(path);
  }
  return next;
}

/** Tick everything in the group except `keep`. */
export function keepOnly(group: DupGroup, sel: Selection, keep: string): Set<string> {
  const next = new Set(sel);
  for (const f of group.files) {
    if (f.path === keep) next.delete(f.path);
    else next.add(f.path);
  }
  return next;
}

/** The selection produced by an auto-select rule, restricted to files that are still listed. */
export function fromAuto(groups: readonly DupGroup[], auto: AutoSelectResult): Set<string> {
  const byId = new Map(groups.map((g) => [g.groupId, g]));
  const out = new Set<string>();
  for (const a of auto.groups) {
    const g = byId.get(a.groupId);
    if (!g) continue;
    const listed = new Set(g.files.map((f) => f.path));
    const picked = a.selected.filter((p) => listed.has(p));
    if (picked.length >= g.files.length) continue; // never trust a rule that selects all
    for (const p of picked) out.add(p);
  }
  return out;
}

/** Ids of groups whose every file is selected. Must be empty before deleting. */
export function blockedGroups(groups: readonly DupGroup[], sel: Selection): string[] {
  return groups.filter((g) => g.files.length > 0 && selectedIn(g, sel) >= g.files.length).map((g) => g.groupId);
}

export function canDelete(groups: readonly DupGroup[], sel: Selection): boolean {
  return selectedPaths(groups, sel).length > 0 && blockedGroups(groups, sel).length === 0;
}

/** Selected paths that belong to the listed groups, in list order. */
export function selectedPaths(groups: readonly DupGroup[], sel: Selection): string[] {
  const out: string[] = [];
  for (const g of groups) for (const f of g.files) if (sel.has(f.path)) out.push(f.path);
  return out;
}

export function selectedBytes(groups: readonly DupGroup[], sel: Selection): number {
  let n = 0;
  for (const g of groups) for (const f of g.files) if (sel.has(f.path)) n += f.bytes;
  return n;
}

/** Number of groups that have something selected. */
export function groupsTouched(groups: readonly DupGroup[], sel: Selection): number {
  return groups.filter((g) => selectedIn(g, sel) > 0).length;
}

/** Drop deleted files; a group left with fewer than two files is no longer a duplicate. */
export function removeFiles(groups: readonly DupGroup[], gone: ReadonlySet<string>): DupGroup[] {
  const out: DupGroup[] = [];
  for (const g of groups) {
    const files = g.files.filter((f) => !gone.has(f.path));
    if (files.length < 2) continue;
    const total = files.reduce((s, f) => s + f.bytes, 0);
    const max = files.reduce((m, f) => Math.max(m, f.bytes), 0);
    out.push({ ...g, files, wastedBytes: total - max });
  }
  return out;
}
