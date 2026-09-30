import type { ReactNode } from 'react';

interface Props {
  title?: string;
  children: ReactNode;
  className?: string;
  testId?: string;
}

export function Card({ title, children, className = '', testId }: Props) {
  // A card whose body runs edge to edge (`!p-0`, list rows with their own padding) still needs
  // its title inset from the border.
  const flush = /(^|\s)!p-0(\s|$)/.test(className);
  return (
    <section
      data-testid={testId}
      className={`min-w-0 rounded-xl border border-line bg-surface p-3 ${className}`}
    >
      {title && (
        <h2 className={`m-0 mb-2 text-xs font-semibold uppercase tracking-wide text-muted ${flush ? 'px-3 pt-3' : ''}`}>
          {title}
        </h2>
      )}
      {children}
    </section>
  );
}
