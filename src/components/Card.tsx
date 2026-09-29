import type { ReactNode } from 'react';

interface Props {
  title?: string;
  children: ReactNode;
  className?: string;
  testId?: string;
}

export function Card({ title, children, className = '', testId }: Props) {
  return (
    <section
      data-testid={testId}
      className={`min-w-0 rounded-xl border border-line bg-surface p-3 ${className}`}
    >
      {title && <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted">{title}</h2>}
      {children}
    </section>
  );
}
