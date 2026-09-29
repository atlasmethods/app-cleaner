import { Hourglass } from 'lucide-react';
import { EmptyState } from './EmptyState';

interface Props {
  title: string;
  testId: string;
}

/** Shared body for pages that are not built yet; later work replaces the page component. */
export function PlaceholderPage({ title, testId }: Props) {
  return (
    <div data-testid={testId} className="p-4">
      <h2 className="text-lg font-semibold">{title}</h2>
      <EmptyState icon={Hourglass} title="Coming soon" hint="This feature is not available yet." />
    </div>
  );
}
