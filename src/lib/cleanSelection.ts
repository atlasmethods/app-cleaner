import type { AnalyzeItem, CategoryInfo, GroupInfo, RuleInfo, RulesListing } from '../api/cleaner';

export type GroupState = 'all' | 'some' | 'none';

export function allRules(listing: RulesListing): RuleInfo[] {
  return listing.categories.flatMap((c) => c.groups.flatMap((g) => g.rules));
}

/** Ids enabled in the listing, as returned by the server. */
export function selectionOf(listing: RulesListing): Set<string> {
  return new Set(allRules(listing).filter((r) => r.enabled).map((r) => r.id));
}

export function groupState(group: GroupInfo, selected: ReadonlySet<string>): GroupState {
  const n = group.rules.filter((r) => selected.has(r.id)).length;
  if (n === 0) return 'none';
  return n === group.rules.length ? 'all' : 'some';
}

export interface TogglePlan {
  /** Whether the toggle turns rules on. */
  on: boolean;
  /** Rule ids whose state changes. */
  ids: string[];
  /** Rules among `ids` that carry a warning (only relevant when `on`). */
  warnings: RuleInfo[];
}

/** A rule toggled by itself. */
export function planRuleToggle(rule: RuleInfo, selected: ReadonlySet<string>): TogglePlan {
  const on = !selected.has(rule.id);
  return { on, ids: [rule.id], warnings: on && rule.warning ? [rule] : [] };
}

/**
 * Clicking a group checkbox: everything on unless everything is already on (then off).
 * Turning on rules that have warnings needs the user's confirmation, so they are
 * reported in `warnings`.
 */
export function planGroupToggle(group: GroupInfo, selected: ReadonlySet<string>): TogglePlan {
  const on = groupState(group, selected) !== 'all';
  const rules = on ? group.rules.filter((r) => !selected.has(r.id)) : group.rules;
  return {
    on,
    ids: rules.map((r) => r.id),
    warnings: on ? rules.filter((r) => r.warning) : [],
  };
}

export function planCategoryToggle(cat: CategoryInfo, selected: ReadonlySet<string>): TogglePlan {
  const rules = cat.groups.flatMap((g) => g.rules);
  const all = rules.every((r) => selected.has(r.id));
  const on = !all;
  const changing = on ? rules.filter((r) => !selected.has(r.id)) : rules;
  return { on, ids: changing.map((r) => r.id), warnings: on ? changing.filter((r) => r.warning) : [] };
}

export function applyPlan(selected: ReadonlySet<string>, plan: TogglePlan): Set<string> {
  const next = new Set(selected);
  for (const id of plan.ids) {
    if (plan.on) next.add(id);
    else next.delete(id);
  }
  return next;
}

/** Stable order for persistence: the listing order, not click order. */
export function orderedSelection(listing: RulesListing, selected: ReadonlySet<string>): string[] {
  return allRules(listing)
    .map((r) => r.id)
    .filter((id) => selected.has(id));
}

// ---------------------------------------------------------------- analysis results

/** Does this analysis item have anything to show or clean? */
export function hasContent(it: AnalyzeItem): boolean {
  return it.files > 0 || it.rows > 0 || it.actions.length > 0;
}

/** Biggest first; ties by files, rows, then name for a stable order. */
export function sortResults(items: AnalyzeItem[]): AnalyzeItem[] {
  return [...items].sort(
    (a, b) =>
      b.bytes - a.bytes ||
      b.files - a.files ||
      b.rows - a.rows ||
      `${a.group} ${a.name}`.localeCompare(`${b.group} ${b.name}`),
  );
}

/** Items worth listing: content, or problems the user should hear about. */
export function visibleResults(items: AnalyzeItem[]): AnalyzeItem[] {
  return sortResults(items.filter((i) => hasContent(i) || i.errors.length > 0));
}

export function slug(s: string): string {
  return s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
}
