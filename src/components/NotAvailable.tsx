import { Info } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

interface Props {
  title: string;
  /** Explanation from the backend. */
  detail?: string;
  /** Commands or steps that would make the feature available. */
  hints?: string[];
  icon?: LucideIcon;
  testId?: string;
}

/**
 * A neutral "this does not work on this machine" state (missing system tool, other OS). It is
 * information, not a failure, so it uses no error colours and no alert role.
 */
export function NotAvailable({ title, detail, hints, icon: Icon = Info, testId = 'not-available' }: Props) {
  return (
    <div
      role="status"
      data-testid={testId}
      className="flex flex-col items-center gap-2 px-4 py-10 text-center"
    >
      <Icon size={36} className="text-muted" aria-hidden />
      <p className="m-0 text-base font-semibold">{title}</p>
      {detail && (
        <p className="m-0 max-w-full break-words text-sm text-muted" data-testid={`${testId}-detail`}>
          {detail}
        </p>
      )}
      {hints && hints.length > 0 && (
        <div className="mt-1 w-full min-w-0 max-w-md text-left" data-testid={`${testId}-hints`}>
          <p className="m-0 mb-1 text-xs text-muted">To enable it, install one of these and scan again:</p>
          <ul className="m-0 flex list-none flex-col gap-1 p-0">
            {hints.map((h) => (
              <li key={h} className="break-all rounded-lg border border-line bg-surface-2 px-2 py-1 font-mono text-xs">
                {h}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
