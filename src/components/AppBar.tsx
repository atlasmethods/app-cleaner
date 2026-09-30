import { ChevronLeft } from 'lucide-react';

interface Props {
  title: string;
  onBack?: () => void;
}

export function AppBar({ title, onBack }: Props) {
  return (
    <header data-testid="appbar" className="shrink-0 border-b border-line bg-surface">
      <div className="mx-auto flex h-12 w-full max-w-[720px] items-center gap-1 px-2">
        {onBack ? (
          <button
            type="button"
            onClick={onBack}
            aria-label="Back"
            data-testid="appbar-back"
            className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full border-0 bg-transparent text-fg hover:bg-surface-2"
          >
            <ChevronLeft size={22} aria-hidden />
          </button>
        ) : (
          <span className="w-2" aria-hidden />
        )}
        <h1 data-testid="appbar-title" className="m-0 min-w-0 flex-1 truncate text-base font-semibold">
          {title}
        </h1>
      </div>
    </header>
  );
}
