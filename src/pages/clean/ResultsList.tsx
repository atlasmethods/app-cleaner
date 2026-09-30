import { ChevronDown, ChevronRight } from 'lucide-react';
import type { AnalyzeItem } from '../../api/cleaner';
import { Card } from '../../components/Card';
import { describeCounts, formatBytes } from '../../lib/format';

interface Props {
  items: AnalyzeItem[];
  expanded: ReadonlySet<string>;
  onToggle: (ruleId: string) => void;
  totalBytes: number;
  totalFiles: number;
  totalRows: number;
}

/** Analysis results; the caller passes them already sorted and filtered. */
export function ResultsList({ items, expanded, onToggle, totalBytes, totalFiles, totalRows }: Props) {
  if (items.length === 0) {
    return (
      <Card title="Results" testId="results">
        <p className="m-0 text-sm text-muted" data-testid="results-empty">
          Nothing to clean for the selected items.
        </p>
      </Card>
    );
  }
  return (
    <Card title="Results" testId="results" className="!p-0">
      <ul className="m-0 list-none divide-y divide-line p-0">
        {items.map((it) => {
          const open = expanded.has(it.ruleId);
          return (
            <li key={it.ruleId} data-testid={`result-${it.ruleId}`} className="min-w-0">
              <button
                type="button"
                aria-expanded={open}
                onClick={() => onToggle(it.ruleId)}
                data-testid={`result-${it.ruleId}-toggle`}
                className="flex w-full min-w-0 items-center gap-2 border-0 bg-transparent px-3 py-2 text-left"
              >
                <span className="min-w-0 flex-1">
                  <span className="block break-words text-sm font-medium">
                    {it.group} <span className="font-normal text-muted">- {it.name}</span>
                  </span>
                  <span className="block text-xs text-muted">
                    {describeCounts(it)}
                    {it.appRunning && <span className="ml-1 text-warn">- app is running</span>}
                  </span>
                </span>
                <span className="shrink-0 text-sm font-semibold" data-testid={`result-${it.ruleId}-size`}>
                  {formatBytes(it.bytes)}
                </span>
                {open ? <ChevronDown size={16} aria-hidden /> : <ChevronRight size={16} aria-hidden />}
              </button>
              {open && (
                <div className="px-3 pb-2" data-testid={`result-${it.ruleId}-details`}>
                  {it.samplePaths.length > 0 && (
                    <ul className="m-0 list-none p-0">
                      {it.samplePaths.map((p) => (
                        <li key={p} className="break-all py-0.5 font-mono text-[11px] text-muted">
                          {p}
                        </li>
                      ))}
                    </ul>
                  )}
                  {(it.inUseSkipped ?? 0) > 0 && (
                    <p className="m-0 mt-1 text-[11px] text-muted" data-testid={`result-${it.ruleId}-in-use`}>
                      {it.inUseSkipped} in-use {it.inUseSkipped === 1 ? 'item was' : 'items were'} left alone.
                    </p>
                  )}
                  {it.errors.length > 0 && (
                    <ul className="m-0 mt-1 list-none p-0" data-testid={`result-${it.ruleId}-errors`}>
                      {it.errors.map((e, i) => (
                        <li key={`${e.path}-${i}`} className="break-all py-0.5 text-[11px] text-danger">
                          {e.path}: {e.message}
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
              )}
            </li>
          );
        })}
      </ul>
      <p className="m-0 border-t border-line px-3 py-2 text-sm" data-testid="results-total">
        Total: <strong>{formatBytes(totalBytes)}</strong> - {describeCounts({ files: totalFiles, rows: totalRows })}
      </p>
    </Card>
  );
}
