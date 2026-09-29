import { X } from 'lucide-react';
import { useEffect, type ReactNode } from 'react';

interface Props {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
  testId?: string;
}

/** Scrollable bottom sheet for option panels and menus (ConfirmSheet is for yes/no questions). */
export function BottomSheet({ open, title, onClose, children, testId }: Props) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div className="fixed inset-0 z-30 flex items-end justify-center" data-testid={testId}>
      <button
        type="button"
        aria-label="Dismiss"
        onClick={onClose}
        className="absolute inset-0 border-0 bg-black/50"
      />
      <div
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className="relative flex max-h-[88dvh] w-full max-w-[480px] flex-col rounded-t-2xl border border-line bg-surface shadow-xl"
      >
        <div className="flex shrink-0 items-center gap-2 border-b border-line px-4 py-2">
          <h2 className="m-0 min-w-0 flex-1 truncate text-base font-semibold">{title}</h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            data-testid={testId ? `${testId}-close` : undefined}
            className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full border-0 bg-transparent"
          >
            <X size={18} aria-hidden />
          </button>
        </div>
        <div className="min-w-0 overflow-y-auto overflow-x-hidden p-4 pb-[calc(1rem+env(safe-area-inset-bottom))]">
          {children}
        </div>
      </div>
    </div>
  );
}
