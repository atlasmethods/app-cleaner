import type { LucideIcon } from 'lucide-react';

interface Props {
  title: string;
  hint?: string;
  icon?: LucideIcon;
}

export function EmptyState({ title, hint, icon: Icon }: Props) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 px-6 py-12 text-center">
      {Icon && <Icon size={36} className="text-muted" aria-hidden />}
      <p className="text-base font-semibold">{title}</p>
      {hint && <p className="text-sm text-muted">{hint}</p>}
    </div>
  );
}
