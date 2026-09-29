import type { LucideIcon } from 'lucide-react';
import { Link } from 'react-router-dom';

interface Props {
  to: string;
  label: string;
  icon: LucideIcon;
  testId: string;
}

export function Tile({ to, label, icon: Icon, testId }: Props) {
  return (
    <Link
      to={to}
      data-testid={testId}
      className="flex min-h-[84px] min-w-0 flex-col items-center justify-center gap-1.5 rounded-xl border border-line bg-surface p-2 text-center text-fg no-underline active:bg-surface-2"
    >
      <Icon size={22} className="text-accent" aria-hidden />
      <span className="w-full break-words text-[11px] font-medium leading-tight">{label}</span>
    </Link>
  );
}
