import { TriangleAlert } from 'lucide-react';
import type { ApiCallError } from '../lib/transport';

interface Props {
  error: ApiCallError | Error | null;
  onRetry?: () => void;
}

export function ErrorBanner({ error, onRetry }: Props) {
  if (!error) return null;
  const code = 'code' in error ? String((error as ApiCallError).code) : undefined;
  return (
    <div
      role="alert"
      data-testid="error-banner"
      className="flex min-w-0 items-start gap-2 rounded-xl border border-danger/40 bg-danger/10 p-3 text-sm"
    >
      <TriangleAlert size={18} className="mt-0.5 shrink-0 text-danger" aria-hidden />
      <div className="min-w-0 flex-1">
        {code && <p className="text-xs font-semibold text-danger">{code}</p>}
        <p className="break-words">{error.message}</p>
      </div>
      {onRetry && (
        <button
          type="button"
          onClick={onRetry}
          className="shrink-0 rounded-lg border border-line bg-surface px-2 py-1 text-xs font-medium"
        >
          Retry
        </button>
      )}
    </div>
  );
}
