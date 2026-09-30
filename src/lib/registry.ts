import type { CategoryInfo, FixResult, Issue } from '../api/registry_cleaner';

export interface IssueGroup {
  category: CategoryInfo;
  issues: Issue[];
}

/** Issues grouped in the order of the category list; unknown categories go last. */
export function groupIssues(issues: Issue[], categories: CategoryInfo[]): IssueGroup[] {
  const byId = new Map<string, Issue[]>();
  for (const i of issues) {
    const list = byId.get(i.category);
    if (list) list.push(i);
    else byId.set(i.category, [i]);
  }
  const groups: IssueGroup[] = [];
  for (const c of categories) {
    const list = byId.get(c.id);
    if (list?.length) {
      groups.push({ category: c, issues: list });
      byId.delete(c.id);
    }
  }
  for (const [id, list] of byId) {
    groups.push({
      category: { id, label: id, description: '', severity: 'low', defaultSelected: false },
      issues: list,
    });
  }
  return groups;
}

export function defaultCategories(categories: CategoryInfo[]): Set<string> {
  return new Set(categories.filter((c) => c.defaultSelected).map((c) => c.id));
}

/** Which issues start ticked: those of categories that are safe to fix without a second look. */
export function preselectIssues(issues: Issue[], categories: CategoryInfo[]): Set<string> {
  const safe = new Set(categories.filter((c) => c.defaultSelected).map((c) => c.id));
  return new Set(issues.filter((i) => safe.has(i.category)).map((i) => i.id));
}

export type CheckState = 'all' | 'some' | 'none';

export function groupState(issues: Issue[], selected: Set<string>): CheckState {
  const n = issues.filter((i) => selected.has(i.id)).length;
  return n === 0 ? 'none' : n === issues.length ? 'all' : 'some';
}

/** Tick or untick every issue in `issues`. */
export function setMany(selected: Set<string>, issues: Issue[], on: boolean): Set<string> {
  const next = new Set(selected);
  for (const i of issues) {
    if (on) next.add(i.id);
    else next.delete(i.id);
  }
  return next;
}

export function toggleOne(selected: Set<string>, id: string): Set<string> {
  const next = new Set(selected);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

/** Only ids that still exist in `issues` (after a re-scan). */
export function pruneSelection(selected: Set<string>, issues: Issue[]): Set<string> {
  const ids = new Set(issues.map((i) => i.id));
  return new Set([...selected].filter((id) => ids.has(id)));
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

export function confirmMessage(count: number, adminCount: number, windows: boolean): string {
  const what = windows ? plural(count, 'registry entry', 'registry entries') : plural(count, 'item');
  let m = `${what} will be fixed. A backup of everything that changes is saved first, and you can restore it later from Backups. If the backup cannot be made, nothing is changed.`;
  if (adminCount > 0) {
    m += ` ${adminCount} of them need administrator rights, so you may be asked to approve once.`;
  }
  return m;
}

export function fixSummary(r: FixResult): string {
  if (r.fixed === 0 && r.failed === 0) return 'Nothing was changed.';
  const parts = [`Fixed ${r.fixed}`];
  if (r.failed > 0) parts.push(`${r.failed} could not be fixed`);
  return `${parts.join(', ')}.`;
}

/** Results of failed items with the issue text attached, for the result card. */
export function failures(r: FixResult, issues: Issue[]): { id: string; description: string; error: string }[] {
  const byId = new Map(issues.map((i) => [i.id, i]));
  return r.results
    .filter((x) => !x.ok)
    .map((x) => ({
      id: x.id,
      description: byId.get(x.id)?.description ?? x.id,
      error: x.error ?? 'Unknown error',
    }));
}

export function pageNoun(platform: string): string {
  return platform === 'windows' ? 'registry' : 'configuration';
}
