import { useRef, type ReactNode } from 'react';
import { useModal } from './useModal';

interface Props {
  open: boolean;
  title: string;
  subtitle?: string;
  onClose: () => void;
  children: ReactNode;
  testId?: string;
}

/**
 * Bottom sheet holding arbitrary content (row menus). Scrolls inside when taller than the
 * viewport, traps focus, closes on Escape / backdrop tap and locks the page behind it.
 * Destructive actions still go through ConfirmSheet.
 */
export function BottomSheet({ open, title, subtitle, onClose, children, testId = 'bottom-sheet' }: Props) {
  const dialog = useRef<HTMLDivElement>(null);
  useModal(open, onClose, dialog);

  if (!open) return null;
  return (
    <div className="fixed inset-0 z-30 flex items-end justify-center" data-testid={testId}>
      <button
        type="button"
        tabIndex={-1}
        aria-label="Close"
        onClick={onClose}
        data-testid={`${testId}-close`}
        className="absolute inset-0 border-0 bg-black/50"
      />
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
        data-testid={`${testId}-dialog`}
        className="relative max-h-[85dvh] w-full max-w-[480px] overflow-y-auto overflow-x-hidden overscroll-contain rounded-t-2xl border border-line bg-surface p-4 pb-[calc(1rem+env(safe-area-inset-bottom))] shadow-xl outline-none"
      >
        <h2 className="m-0 break-words text-base font-semibold" data-testid={`${testId}-title`}>
          {title}
        </h2>
        {subtitle && <p className="m-0 mt-0.5 break-words text-xs text-muted">{subtitle}</p>}
        <div className="mt-3 flex flex-col gap-2">{children}</div>
      </div>
    </div>
  );
}
