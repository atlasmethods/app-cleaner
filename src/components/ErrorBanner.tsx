import { Info, TriangleAlert } from 'lucide-react';
import type { ApiCallError } from '../lib/transport';

interface Props {
  error: ApiCallError | Error | null;
  onRetry?: () => void;
}

export function ErrorBanner({ error, onRetry }: Props) {
  if (!error) return null;
  const code = 'code' in error ? String((error as ApiCallError).code) : undefined;
  // Unsupported means "not possible on this machine" (missing tool, other OS): information,
  // not a failure, so it gets a neutral notice instead of the red alert.
  if (code === 'Unsupported') {
    return (
      <div
        role="status"
        data-testid="info-banner"
        className="flex min-w-0 items-start gap-2 rounded-xl border border-line bg-surface-2 p-3 text-sm"
      >
        <Info size={18} className="mt-0.5 shrink-0 text-muted" aria-hidden />
        <div className="min-w-0 flex-1">
          <p className="m-0 text-xs font-semibold text-muted">Not available here</p>
          <p className="m-0 break-words">{error.message}</p>
        </div>
      </div>
    );
  }
  const unreachable = code === 'Unreachable';
  // Nothing on the page can work without the backend, so a reload is the natural retry.
  const retry = onRetry ?? (unreachable ? () => window.location.reload() : undefined);
  return (
    <div
      role="alert"
      data-testid="error-banner"
      className="flex min-w-0 items-start gap-2 rounded-xl border border-danger/40 bg-danger/10 p-3 text-sm"
    >
      <TriangleAlert size={18} className="mt-0.5 shrink-0 text-danger" aria-hidden />
      <div className="min-w-0 flex-1">
        {code && (
          <p className="m-0 text-xs font-semibold text-danger">{unreachable ? 'Backend unreachable' : code}</p>
        )}
        <p className="m-0 break-words">{error.message}</p>
      </div>
      {retry && (
        <button
          type="button"
          onClick={retry}
          data-testid="error-retry"
          className="h-10 shrink-0 rounded-lg border border-line bg-surface px-3 text-xs font-medium"
        >
          Retry
        </button>
      )}
    </div>
  );
}
