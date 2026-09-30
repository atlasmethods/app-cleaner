import { useId, useRef, useState } from 'react';
import { useModal } from './useModal';

interface Props {
  open: boolean;
  title: string;
  message?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Style the confirm button as destructive. */
  danger?: boolean;
  /** Strong confirmation: the confirm button stays disabled until this exact text is typed. */
  requireText?: string;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * Bottom-sheet confirmation dialog. Focus starts on Cancel (the safe choice), stays inside
 * the sheet, and Escape cancels. A long message scrolls inside the sheet.
 */
export function ConfirmSheet(props: Props) {
  // The body only exists while open, so typed text and focus start fresh every time.
  return props.open ? <OpenSheet {...props} /> : null;
}

function OpenSheet({
  title,
  message,
  confirmLabel = 'Confirm',
  cancelLabel = 'Cancel',
  danger = false,
  requireText,
  onConfirm,
  onCancel,
}: Props) {
  const dialog = useRef<HTMLDivElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const [typed, setTyped] = useState('');
  const inputId = useId();
  useModal(true, onCancel, dialog, cancel);

  const locked = requireText !== undefined && typed !== requireText;
  return (
    <div className="fixed inset-0 z-40 flex items-end justify-center" data-testid="confirm-sheet">
      <button
        type="button"
        tabIndex={-1}
        aria-label="Dismiss"
        onClick={onCancel}
        className="absolute inset-0 border-0 bg-black/50"
      />
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-title"
        aria-describedby={message ? 'confirm-message' : undefined}
        tabIndex={-1}
        className="relative flex max-h-[85dvh] w-full max-w-[480px] flex-col overflow-hidden rounded-t-2xl border border-line bg-surface p-4 pb-[calc(1rem+env(safe-area-inset-bottom))] shadow-xl outline-none"
      >
        <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
          <h2 id="confirm-title" className="m-0 break-words text-base font-semibold">
            {title}
          </h2>
          {message && (
            <p id="confirm-message" className="m-0 mt-1 break-words text-sm text-muted">
              {message}
            </p>
          )}
        </div>
        {requireText !== undefined && (
          <div className="mt-3 shrink-0">
            <label htmlFor={inputId} className="block text-sm">
              Type <span className="font-mono font-semibold">{requireText}</span> to confirm
            </label>
            <input
              id={inputId}
              type="text"
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              autoComplete="off"
              autoCapitalize="off"
              spellCheck={false}
              data-testid="confirm-sheet-input"
              className="mt-1 h-11 w-full min-w-0 rounded-xl border border-line bg-surface-2 px-3 font-mono text-sm"
            />
          </div>
        )}
        <div className="mt-4 flex shrink-0 gap-2">
          <button
            ref={cancel}
            type="button"
            onClick={onCancel}
            data-testid="confirm-sheet-cancel"
            className="h-11 flex-1 rounded-xl border border-line bg-surface-2 text-sm font-medium"
          >
            {cancelLabel}
          </button>
          <button
            type="button"
            onClick={onConfirm}
            disabled={locked}
            data-testid="confirm-sheet-confirm"
            className={`h-11 flex-1 rounded-xl border-0 text-sm font-semibold disabled:opacity-50 ${
              danger ? 'bg-danger text-danger-fg' : 'bg-accent text-accent-fg'
            }`}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
