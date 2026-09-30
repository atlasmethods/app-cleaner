import { useEffect } from 'react';
import { History as HistoryIcon } from 'lucide-react';
import type { HistoryEntry, HistorySource } from '../api/cleaner';
import { Card } from '../components/Card';
import { EmptyState } from '../components/EmptyState';
import { ErrorBanner } from '../components/ErrorBanner';
import { useCall } from '../hooks/useCall';
import { formatBytes, formatWhen } from '../lib/format';

const SOURCE_LABEL: Record<HistorySource, string> = {
  manual: 'Manual',
  auto: 'Automatic',
  smart: 'Smart cleaning',
  scheduled: 'Scheduled',
  health: 'Health check',
};

export default function CleanHistoryPage() {
  const { data, error, loading, run } = useCall<HistoryEntry[]>('cleaner.history');

  useEffect(() => {
    void run();
  }, [run]);

  return (
    <div data-testid="page-clean-history" className="flex flex-col gap-3 p-4">
      <ErrorBanner error={error} onRetry={() => void run()} />
      {!data && loading && <p className="py-8 text-center text-sm text-muted">Loading history...</p>}
      {data && data.length === 0 && (
        <EmptyState icon={HistoryIcon} title="No cleaning history yet" hint="Runs show up here after you clean." />
      )}
      {data?.map((h) => (
        <Card key={h.id} testId={`history-${h.id}`}>
          <div className="flex items-baseline justify-between gap-2">
            <span className="min-w-0 break-words text-sm font-semibold">{formatWhen(h.at)}</span>
            <span className="shrink-0 text-sm font-semibold">{formatBytes(h.totalBytes)}</span>
          </div>
          <p className="m-0 mt-0.5 text-xs text-muted">
            {SOURCE_LABEL[h.source]} - {h.totalFiles} {h.totalFiles === 1 ? 'file' : 'files'}
            {h.totalRows > 0 ? `, ${h.totalRows} entries` : ''} - {h.ruleIds.length}{' '}
            {h.ruleIds.length === 1 ? 'item' : 'items'}
          </p>
        </Card>
      ))}
    </div>
  );
}
