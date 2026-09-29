import { useEffect, type ReactNode } from 'react';

interface Props {
  open: boolean;
  title: string;
  subtitle?: string;
  onClose: () => void;
  children: ReactNode;
  testId?: string;
}

/** Bottom sheet holding arbitrary content (row menus). Destructive actions still go through ConfirmSheet. */
export function BottomSheet({ open, title, subtitle, onClose, children, testId = 'bottom-sheet' }: Props) {
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
        aria-label="Close"
        onClick={onClose}
        data-testid={`${testId}-close`}
        className="absolute inset-0 border-0 bg-black/50"
      />
      <div
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className="relative max-h-[85dvh] w-full max-w-[480px] overflow-y-auto overflow-x-hidden rounded-t-2xl border border-line bg-surface p-4 pb-[calc(1rem+env(safe-area-inset-bottom))] shadow-xl"
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
