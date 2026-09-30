import { Lock, Pin } from 'lucide-react';
import { useState } from 'react';
import type { DupGroup } from '../../api/duplicates';
import { Checkbox } from '../../components/Checkbox';
import { formatDate, splitPath } from '../../lib/diskView';
import { canSelect, groupState, keepOnly, toggleFile } from '../../lib/dupSelection';
import { formatBytes } from '../../lib/format';

interface Props {
  groups: DupGroup[];
  selected: ReadonlySet<string>;
  onChange: (next: Set<string>) => void;
}

const STEP = 25;

export function GroupList({ groups, selected, onChange }: Props) {
  const [shown, setShown] = useState(STEP);
  return (
    <div className="flex min-w-0 flex-col gap-2" data-testid="dup-groups">
      {groups.slice(0, shown).map((g) => {
        const st = groupState(g, selected);
        const title = g.key.name ?? splitPath(g.files[0]?.path ?? '').name;
        return (
          <section
            key={g.groupId}
            data-testid={`dup-group-${g.groupId}`}
            className="min-w-0 overflow-hidden rounded-xl border border-line bg-surface"
          >
            <header className="border-b border-line bg-surface-2 px-3 py-2">
              <p className="m-0 break-all text-sm font-semibold">{title}</p>
              <p className="m-0 text-[11px] text-muted" data-testid={`dup-group-${g.groupId}-summary`}>
                {g.files.length} files
                {g.key.bytes !== undefined ? ` of ${formatBytes(g.key.bytes)}` : ''} - wastes{' '}
                {formatBytes(g.wastedBytes)}
                {st.selected > 0 ? ` - ${st.selected} selected` : ''}
              </p>
            </header>
            <ul className="m-0 list-none divide-y divide-line p-0">
              {g.files.map((f, i) => {
                const on = selected.has(f.path);
                const locked = !on && !canSelect(g, selected, f.path);
                const { dir, name } = splitPath(f.path);
                return (
                  <li
                    key={f.path}
                    data-testid="dup-file"
                    data-path={f.path}
                    className="flex min-w-0 items-center gap-2 px-2 py-1.5"
                  >
                    <Checkbox
                      checked={on}
                      disabled={locked}
                      onChange={() => onChange(toggleFile(g, selected, f.path))}
                      ariaLabel={locked ? `${name} (kept: at least one copy stays)` : `Select ${name}`}
                      testId="dup-check"
                    />
                    <span className="min-w-0 flex-1">
                      <span className="block break-all text-sm">{name}</span>
                      <span className="block break-all text-[11px] text-muted">{dir}</span>
                      <span className="block text-[11px] text-muted">
                        {formatBytes(f.bytes)} - {formatDate(f.modified)}
                      </span>
                    </span>
                    {locked && <Lock size={14} className="shrink-0 text-muted" aria-label="Kept" />}
                    <button
                      type="button"
                      onClick={() => onChange(keepOnly(g, selected, f.path))}
                      aria-label={`Keep only ${name} (${i + 1}) and select the other copies`}
                      title="Keep only this one"
                      data-testid="dup-keep-only"
                      className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg border-0 bg-transparent"
                    >
                      <Pin size={15} className="text-muted" aria-hidden />
                    </button>
                  </li>
                );
              })}
            </ul>
          </section>
        );
      })}
      {groups.length > shown && (
        <button
          type="button"
          onClick={() => setShown((n) => n + STEP)}
          data-testid="dup-show-more"
          className="h-10 rounded-xl border border-line bg-surface-2 text-sm font-medium"
        >
          Show more groups ({groups.length - shown} left)
        </button>
      )}
    </div>
  );
}
