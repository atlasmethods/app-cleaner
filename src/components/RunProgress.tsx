import type { ProgressEvent } from '../api/types';
import { Card } from './Card';
import { ProgressBar } from './ProgressBar';

interface Props {
  /** What is running, e.g. "Analyzing". */
  label: string;
  progress: ProgressEvent | null;
  onCancel: () => void;
  testId: string;
  /** Extra line under the bar (for example a warning about cancelling). */
  note?: string;
}

/** Progress bar + status line + Cancel button for one long-running call. */
export function RunProgress({ label, progress, onCancel, testId, note }: Props) {
  return (
    <Card testId={testId}>
      <p className="m-0 mb-2 break-words text-xs text-muted" data-testid={`${testId}-message`}>
        {label}
        {progress?.message ? `: ${progress.message}` : '...'}
      </p>
      {progress?.fraction !== undefined ? (
        <ProgressBar value={progress.fraction * 100} label={`${label} progress`} />
      ) : (
        <div
          role="progressbar"
          aria-label={`${label} progress`}
          className="h-2 w-full animate-pulse overflow-hidden rounded-full bg-surface-2"
        >
          <div className="h-full w-1/3 rounded-full bg-accent" />
        </div>
      )}
      {note && <p className="m-0 mt-2 break-words text-xs text-warn">{note}</p>}
      <button
        type="button"
        onClick={onCancel}
        data-testid="btn-cancel"
        className="mt-2 h-10 w-full rounded-xl border border-line bg-surface-2 text-sm font-medium"
      >
        Cancel
      </button>
    </Card>
  );
}
