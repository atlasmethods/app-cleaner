import { Trash2 } from 'lucide-react';
import type { Leftover } from '../../api/uninstall';
import { Card } from '../../components/Card';
import { Checkbox } from '../../components/Checkbox';
import { formatBytes } from '../../lib/format';

interface Props {
  appName: string;
  items: readonly Leftover[];
  selected: ReadonlySet<string>;
  busy: boolean;
  onToggle: (path: string) => void;
  onRemove: () => void;
  onDismiss: () => void;
}

/** Leftover folders found after an uninstall; nothing is removed until the user confirms. */
export function LeftoversCard({ appName, items, selected, busy, onToggle, onRemove, onDismiss }: Props) {
  const total = items.filter((i) => selected.has(i.path)).reduce((s, i) => s + i.sizeBytes, 0);
  return (
    <Card title={`Leftovers of ${appName}`} testId="leftovers-card" className="!p-0">
      <p className="m-0 px-3 pt-1 text-xs text-muted">
        Folders and files the program left behind. Untick anything you want to keep.
      </p>
      <ul className="m-0 mt-1 list-none divide-y divide-line p-0">
        {items.map((l, i) => (
          <li key={l.path} className="flex min-w-0 items-start gap-2 px-3 py-1.5" data-testid={`leftover-${i}`}>
            <span className="mt-0.5">
              <Checkbox
                checked={selected.has(l.path)}
                onChange={() => onToggle(l.path)}
                ariaLabel={`Select ${l.path}`}
                testId={`leftover-check-${i}`}
              />
            </span>
            <span className="min-w-0 flex-1">
              <span className="block break-all text-xs">{l.path}</span>
              <span className="block text-[11px] text-muted">
                {l.kind} - {formatBytes(l.sizeBytes)}
              </span>
            </span>
          </li>
        ))}
      </ul>
      <div className="flex gap-2 border-t border-line p-2">
        <button
          type="button"
          onClick={onDismiss}
          data-testid="btn-leftovers-dismiss"
          className="h-10 flex-1 rounded-xl border border-line bg-surface-2 text-sm font-medium"
        >
          Keep
        </button>
        <button
          type="button"
          onClick={onRemove}
          disabled={busy || selected.size === 0}
          data-testid="btn-remove-leftovers"
          className="flex h-10 min-w-0 flex-[2] items-center justify-center gap-1.5 rounded-xl border-0 bg-danger text-sm font-semibold text-danger-fg disabled:opacity-50"
        >
          <Trash2 size={16} aria-hidden /> Remove ({formatBytes(total)})
        </button>
      </div>
    </Card>
  );
}
