import type { ReactNode } from 'react';
import { CircleAlert, CircleCheck, CircleSlash, TriangleAlert } from 'lucide-react';
import type { FixReport, PartStatus } from '../../api/health';
import { Card } from '../../components/Card';
import { blockedBrowsers, describePart, failedItems, fixHeadline, PART_LABEL, retryParams } from '../../lib/health';

const ICON = {
  done: <CircleCheck size={16} className="mt-0.5 shrink-0 text-ok" aria-hidden />,
  partial: <TriangleAlert size={16} className="mt-0.5 shrink-0 text-warn" aria-hidden />,
  failed: <CircleAlert size={16} className="mt-0.5 shrink-0 text-danger" aria-hidden />,
  not_run: <CircleSlash size={16} className="mt-0.5 shrink-0 text-muted" aria-hidden />,
} satisfies Record<PartStatus, ReactNode>;

interface Props {
  result: FixReport;
  /** Score before the fix and after it (the fresh re-scan). */
  before: number | null;
  after: number | null;
  onCloseAndFix: () => void;
  busy: boolean;
}

/** The outcome of "Fix all": new score, a "What was done" list, and the running-browser retry. */
export function FixResultCard({ result, before, after, onCloseAndFix, busy }: Props) {
  const blocked = blockedBrowsers(result);
  const canRetry = retryParams(result) !== null;
  return (
    <Card testId="health-result">
      <h2 className="m-0 text-base font-semibold" data-testid="health-result-title">
        {fixHeadline(result)}
      </h2>
      {after !== null && (
        <p className="m-0 mt-0.5 text-sm" data-testid="health-result-score">
          {before !== null && before !== after ? `Score ${before} → ${after}` : `Score ${after}`}
        </p>
      )}
      <h3 className="mb-1 mt-3 text-xs font-semibold uppercase tracking-wide text-muted">What was done</h3>
      <ul className="m-0 flex list-none flex-col gap-2 p-0" data-testid="health-done-list">
        {result.parts.map((p) => (
          <li key={p.part} data-testid={`done-${p.part}`} data-status={p.status} className="flex min-w-0 items-start gap-2">
            {ICON[p.status]}
            <span className="min-w-0 flex-1">
              <span className="block text-sm font-medium">{PART_LABEL[p.part]}</span>
              <span className="block break-words text-xs text-muted">{describePart(p)}</span>
              {failedItems(p).map((i) => (
                <span key={i.id} className="block break-words text-xs text-danger">
                  {i.name}: {i.message}
                </span>
              ))}
            </span>
          </li>
        ))}
      </ul>
      {canRetry && (
        <div className="mt-3 rounded-lg border border-warn/40 bg-warn/10 p-2" data-testid="health-blocked">
          <p className="m-0 break-words text-xs">
            {blocked.join(', ')} {blocked.length === 1 ? 'is' : 'are'} running, so its data was not cleaned.
          </p>
          <button
            type="button"
            onClick={onCloseAndFix}
            disabled={busy}
            data-testid="btn-close-and-fix"
            className="mt-2 h-9 w-full rounded-xl border border-line bg-surface-2 text-sm font-semibold disabled:opacity-50"
          >
            Close browsers &amp; fix
          </button>
        </div>
      )}
    </Card>
  );
}
