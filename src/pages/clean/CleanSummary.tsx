import { CheckCircle2 } from 'lucide-react';
import { Link } from 'react-router-dom';
import type { CleanReport, RuleClean } from '../../api/cleaner';
import { Card } from '../../components/Card';
import { CLEAN_HISTORY_PATH } from '../../nav';
import { describeCounts, formatBytes } from '../../lib/format';

interface Props {
  report: CleanReport;
  /** ruleId -> "Group - Name" for display. */
  names: Record<string, string>;
}

function skippedText(r: RuleClean): string {
  switch (r.skipped) {
    case 'app_running':
      return `skipped - ${r.runningApps.join(', ') || 'the app'} is running`;
    case 'in_use':
      return 'skipped - files are in use';
    case 'unsupported':
      return 'skipped - not supported on this system';
    default:
      return '';
  }
}

export function CleanSummary({ report, names }: Props) {
  const done = report.results.filter((r) => !r.skipped && (r.removedFiles > 0 || r.removedRows > 0 || r.actions.length > 0));
  const skipped = report.results.filter((r) => r.skipped);
  const failed = report.results.filter((r) => r.failed.length > 0);
  return (
    <Card testId="clean-summary">
      <div className="flex items-center gap-2">
        <CheckCircle2 size={20} className="shrink-0 text-ok" aria-hidden />
        <p className="m-0 text-sm font-semibold" data-testid="clean-summary-total">
          {report.cancelled ? 'Cancelled - ' : ''}
          Removed {formatBytes(report.totalBytes)} ({describeCounts({ files: report.totalFiles, rows: report.totalRows }) || 'nothing'})
        </p>
      </div>
      {done.length > 0 && (
        <ul className="m-0 mt-2 list-none p-0">
          {done.map((r) => (
            <li key={r.ruleId} data-testid={`summary-${r.ruleId}`} className="flex justify-between gap-2 py-0.5 text-xs">
              <span className="min-w-0 break-words">{names[r.ruleId] ?? r.ruleId}</span>
              <span className="shrink-0 text-muted">
                {formatBytes(r.removedBytes)}
                {r.closedApps.length > 0 ? ` - closed ${r.closedApps.join(', ')}` : ''}
              </span>
            </li>
          ))}
        </ul>
      )}
      {skipped.length > 0 && (
        <ul className="m-0 mt-2 list-none p-0" data-testid="clean-summary-skipped">
          {skipped.map((r) => (
            <li key={r.ruleId} className="py-0.5 text-xs text-warn">
              {names[r.ruleId] ?? r.ruleId}: {skippedText(r)}
            </li>
          ))}
        </ul>
      )}
      {failed.length > 0 && (
        <ul className="m-0 mt-2 list-none p-0" data-testid="clean-summary-failed">
          {failed.map((r) => (
            <li key={r.ruleId} className="py-0.5 text-xs text-danger">
              {names[r.ruleId] ?? r.ruleId}: {r.failed.length} could not be removed - {r.failed[0]?.path}:{' '}
              {r.failed[0]?.message}
            </li>
          ))}
        </ul>
      )}
      <Link to={CLEAN_HISTORY_PATH} data-testid="history-link" className="mt-3 inline-block text-sm text-accent-strong">
        View cleaning history
      </Link>
    </Card>
  );
}
