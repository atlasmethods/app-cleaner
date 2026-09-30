import { useState } from 'react';
import type { Issue } from '../../api/registry_cleaner';
import { Card } from '../../components/Card';
import { Checkbox } from '../../components/Checkbox';
import { groupState, setMany, toggleOne, type IssueGroup } from '../../lib/registry';

const SHOWN = 100;

interface Props {
  groups: IssueGroup[];
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
  disabled?: boolean;
}

function Badge({ children, tone }: { children: string; tone: 'warn' | 'muted' }) {
  return (
    <span
      className={`shrink-0 rounded-full px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide ${
        tone === 'warn' ? 'bg-warn/10 text-warn' : 'bg-surface-2 text-muted'
      }`}
    >
      {children}
    </span>
  );
}

/** Issues grouped by category, each group and each row with its own checkbox. */
export function IssueGroups({ groups, selected, onChange, disabled }: Props) {
  const [limits, setLimits] = useState<Record<string, number>>({});
  return (
    <div className="flex flex-col gap-3" data-testid="registry-groups">
      {groups.map(({ category, issues }) => {
        const state = groupState(issues, selected);
        const limit = limits[category.id] ?? SHOWN;
        return (
          <Card key={category.id} testId={`registry-group-${category.id}`} className="!p-0">
            <label className="flex min-h-11 cursor-pointer items-center gap-2 border-b border-line px-3 py-2">
              <Checkbox
                checked={state === 'all'}
                indeterminate={state === 'some'}
                onChange={(on) => onChange(setMany(selected, issues, on))}
                ariaLabel={`Select all: ${category.label}`}
                testId={`registry-group-check-${category.id}`}
                disabled={disabled}
              />
              <span className="min-w-0 flex-1 break-words text-sm font-semibold">{category.label}</span>
              <span className="shrink-0 text-xs text-muted" data-testid={`registry-group-count-${category.id}`}>
                {issues.length}
              </span>
            </label>
            <ul className="m-0 list-none divide-y divide-line p-0">
              {issues.slice(0, limit).map((i) => (
                <IssueRow
                  key={i.id}
                  issue={i}
                  checked={selected.has(i.id)}
                  disabled={disabled}
                  onToggle={() => onChange(toggleOne(selected, i.id))}
                />
              ))}
            </ul>
            {issues.length > limit && (
              <div className="border-t border-line p-2">
                <button
                  type="button"
                  onClick={() => setLimits((l) => ({ ...l, [category.id]: limit + 200 }))}
                  data-testid={`registry-more-${category.id}`}
                  className="h-10 w-full rounded-xl border border-line bg-surface-2 text-sm"
                >
                  Show more ({issues.length - limit})
                </button>
              </div>
            )}
          </Card>
        );
      })}
    </div>
  );
}

function IssueRow({
  issue,
  checked,
  disabled,
  onToggle,
}: {
  issue: Issue;
  checked: boolean;
  disabled?: boolean;
  onToggle: () => void;
}) {
  return (
    <li data-testid={`issue-${issue.id}`}>
      <label className="flex cursor-pointer items-start gap-2 px-3 py-2">
        <span className="mt-0.5">
          <Checkbox
            checked={checked}
            onChange={onToggle}
            ariaLabel={issue.description}
            testId={`issue-check-${issue.id}`}
            disabled={disabled}
          />
        </span>
        <span className="min-w-0 flex-1">
          <span className="block break-words text-sm" data-testid="issue-description">
            {issue.description}
          </span>
          <span className="block break-all text-[11px] text-muted" data-testid="issue-location">
            {issue.location}
            {issue.value ? ` : ${issue.value}` : ''}
          </span>
          {(issue.severity === 'medium' || issue.needsAdmin) && (
            <span className="mt-1 flex flex-wrap gap-1">
              {issue.severity === 'medium' && <Badge tone="warn">Check first</Badge>}
              {issue.needsAdmin && <Badge tone="muted">Needs admin</Badge>}
            </span>
          )}
        </span>
      </label>
    </li>
  );
}
