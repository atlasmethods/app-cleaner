import { ChevronDown, ChevronRight, TriangleAlert } from 'lucide-react';
import type { CategoryInfo, GroupInfo, RuleInfo } from '../../api/cleaner';
import { Card } from '../../components/Card';
import { Checkbox } from '../../components/Checkbox';
import { groupState, slug } from '../../lib/cleanSelection';

interface Props {
  categories: CategoryInfo[];
  selected: ReadonlySet<string>;
  expanded: ReadonlySet<string>;
  onToggleRule: (rule: RuleInfo) => void;
  onToggleGroup: (group: GroupInfo) => void;
  onToggleExpand: (group: string) => void;
}

/** Category > group > rule selection tree. */
export function RuleTree({ categories, selected, expanded, onToggleRule, onToggleGroup, onToggleExpand }: Props) {
  return (
    <div className="flex flex-col gap-3" data-testid="rule-tree">
      {categories.map((cat) => (
        <section key={cat.category} data-testid={`category-${cat.category}`} className="min-w-0">
          <h2 className="mb-1 px-1 text-xs font-semibold uppercase tracking-wide text-muted">{cat.label}</h2>
          <Card className="!p-0">
            <ul className="m-0 list-none divide-y divide-line p-0">
              {cat.groups.map((g) => {
                const s = slug(g.group);
                const state = groupState(g, selected);
                const open = expanded.has(g.group);
                const count = g.rules.filter((r) => selected.has(r.id)).length;
                return (
                  <li key={g.group} data-testid={`group-${s}`} className="min-w-0">
                    <div className="flex min-w-0 items-center gap-2 px-3 py-1.5">
                      <Checkbox
                        checked={state === 'all'}
                        indeterminate={state === 'some'}
                        onChange={() => onToggleGroup(g)}
                        ariaLabel={`Select all ${g.group}`}
                        testId={`group-check-${s}`}
                      />
                      <button
                        type="button"
                        aria-expanded={open}
                        onClick={() => onToggleExpand(g.group)}
                        data-testid={`group-expand-${s}`}
                        className="flex min-h-9 min-w-0 flex-1 items-center gap-1 border-0 bg-transparent p-0 text-left text-sm font-medium"
                      >
                        <span className="min-w-0 flex-1 break-words">{g.group}</span>
                        <span className="shrink-0 text-xs text-muted">
                          {count}/{g.rules.length}
                        </span>
                        {open ? <ChevronDown size={16} aria-hidden /> : <ChevronRight size={16} aria-hidden />}
                      </button>
                    </div>
                    {open && (
                      <ul className="m-0 mb-1 list-none p-0">
                        {g.rules.map((r) => (
                          <li key={r.id} className="min-w-0">
                            <label className="flex min-w-0 cursor-pointer items-start gap-2 py-1.5 pl-10 pr-3">
                              <span className="mt-0.5">
                                <Checkbox
                                  checked={selected.has(r.id)}
                                  onChange={() => onToggleRule(r)}
                                  ariaLabel={`${g.group}: ${r.name}`}
                                  testId={`rule-${r.id}`}
                                />
                              </span>
                              <span className="min-w-0 flex-1">
                                <span className="block break-words text-sm">
                                  {r.name}
                                  {r.warning && (
                                    <TriangleAlert
                                      size={13}
                                      className="ml-1 inline text-warn"
                                      aria-label="Has a warning"
                                    />
                                  )}
                                </span>
                                <span className="block break-words text-xs text-muted">{r.description}</span>
                              </span>
                            </label>
                          </li>
                        ))}
                      </ul>
                    )}
                  </li>
                );
              })}
            </ul>
          </Card>
        </section>
      ))}
    </div>
  );
}
