import { KeyRound } from 'lucide-react';

/**
 * Shown instead of the app when the local server rejects every request (first API call or
 * heartbeat returns 401): this tab was opened without the private link, or from an old launch.
 */
export default function AuthRequiredPage() {
  return (
    <main
      data-testid="page-auth-required"
      className="mx-auto flex min-h-dvh w-full max-w-[480px] flex-col items-center justify-center gap-3 px-6 py-10 text-center"
    >
      <KeyRound size={40} className="text-accent" aria-hidden />
      <h1 className="m-0 text-xl font-semibold">Open ClearSweep from its link</h1>
      <p className="m-0 text-sm">
        This tab does not have permission to use ClearSweep. Open it from the link printed in your terminal when you ran{' '}
        <code className="rounded bg-surface-2 px-1 py-0.5 font-mono text-xs">clearsweep ui</code>, or from your launcher.
      </p>
      <p className="m-0 text-xs text-muted">
        The link contains a private key that changes every time ClearSweep starts, so links from earlier runs stop working.
        Nothing on your computer was changed.
      </p>
      <button
        type="button"
        onClick={() => window.location.reload()}
        data-testid="auth-retry"
        className="mt-2 h-11 rounded-xl border border-line bg-surface-2 px-6 text-sm font-medium"
      >
        Try again
      </button>
    </main>
  );
}
