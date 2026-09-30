import { Compass } from 'lucide-react';
import { Link, useLocation } from 'react-router-dom';
import { EmptyState } from '../components/EmptyState';

/** Unknown route (a mistyped or outdated `#/...` link). */
export default function NotFoundPage() {
  const { pathname } = useLocation();
  return (
    <div data-testid="page-not-found" className="flex flex-col items-center gap-3 p-4">
      <EmptyState
        icon={Compass}
        title="Page not found"
        hint={`There is nothing at ${pathname}. The link may be mistyped or from an older version.`}
      />
      <Link
        to="/"
        data-testid="not-found-home"
        className="flex h-11 items-center justify-center rounded-xl bg-accent px-6 text-sm font-semibold text-accent-fg no-underline"
      >
        Go to Home
      </Link>
    </div>
  );
}
