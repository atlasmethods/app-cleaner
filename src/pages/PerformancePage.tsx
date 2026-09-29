import { useCallback, useEffect, useMemo, useState } from 'react';
import { Lock, Moon, RefreshCw, Sun } from 'lucide-react';
import type { AppResults, OptimizerAnalysis, OptimizerApp, SleepResult, WakeResult } from '../api/optimizer';
import { Card } from '../components/Card';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { EmptyState } from '../components/EmptyState';
import { ErrorBanner } from '../components/ErrorBanner';
import { RunProgress } from '../components/RunProgress';
import { Switch } from '../components/Switch';
import { useCall } from '../hooks/useCall';
import { activeApps, appSubline, sleepable, sleepingApps, sleepReport, summarize, wakeReport } from '../lib/optimizer';

type Pending = { kind: 'one'; app: OptimizerApp } | { kind: 'all'; apps: OptimizerApp[] };

export default function PerformancePage() {
  const analyze = useCall<OptimizerAnalysis>('optimizer.analyze');
  const sleep = useCall<AppResults<SleepResult>, { appIds: string[] }>('optimizer.sleep');
  const wake = useCall<AppResults<WakeResult>, { appIds: string[] }>('optimizer.wake');
  const [pending, setPending] = useState<Pending | null>(null);
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);

  const run = analyze.run;
  useEffect(() => {
    void run();
  }, [run]);

  const apps = analyze.data?.apps ?? null;
  const active = useMemo(() => (apps ? activeApps(apps) : []), [apps]);
  const asleep = useMemo(() => (apps ? sleepingApps(apps) : []), [apps]);
  const canSleep = useMemo(() => (apps ? sleepable(apps) : []), [apps]);
  const summary = useMemo(() => summarize(apps ?? []), [apps]);
  const busy = sleep.loading || wake.loading;

  const doSleep = useCallback(
    async (ids: string[]) => {
      setPending(null);
      setNote(null);
      const r = await sleep.run({ appIds: ids });
      if (r) setNote(sleepReport(r.results));
      await run();
    },
    [sleep, run],
  );

  const doWake = useCallback(
    async (ids: string[]) => {
      setNote(null);
      const r = await wake.run({ appIds: ids });
      if (r) setNote(wakeReport(r.results));
      await run();
    },
    [wake, run],
  );

  const confirmCopy = ((): { title: string; message: string; label: string } => {
    if (!pending) return { title: '', message: '', label: 'Sleep' };
    if (pending.kind === 'one')
      return {
        title: `Put ${pending.app.name} to sleep?`,
        message:
          pending.app.processes.length > 0
            ? `${pending.app.name} is asked to close (save your work in it first) and will not start with your computer until you wake it.`
            : `${pending.app.name} will not start with your computer until you wake it.`,
        label: 'Sleep',
      };
    const names = pending.apps.map((a) => a.name);
    return {
      title: `Put ${pending.apps.length} ${pending.apps.length === 1 ? 'app' : 'apps'} to sleep?`,
      message: `${names.slice(0, 8).join(', ')}${names.length > 8 ? ', ...' : ''}. They are asked to close (save your work first) and will not start with your computer until you wake them.`,
      label: 'Sleep all',
    };
  })();

  const error = analyze.error ?? sleep.error ?? wake.error;

  return (
    <div data-testid="page-performance" className="flex min-h-full flex-col gap-3 p-4">
      <ErrorBanner error={error} onRetry={analyze.error ? () => void run() : undefined} />

      <Card testId="perf-summary">
        <div className="flex items-start justify-between gap-2">
          <div className="min-w-0">
            <p className="m-0 text-xs font-semibold uppercase tracking-wide text-muted">Background apps</p>
            {apps === null ? (
              <p className="m-0 mt-1 text-sm text-muted" data-testid="perf-loading">
                Looking at what is running...
              </p>
            ) : (
              <p className="m-0 mt-1 break-words text-sm" data-testid="perf-summary-text">
                <span className="text-2xl font-semibold" data-testid="perf-summary-count">
                  {summary.runningApps}
                </span>{' '}
                {summary.runningApps === 1 ? 'app is' : 'apps are'} running and use{' '}
                <span className="font-semibold" data-testid="perf-summary-memory">
                  {summary.memoryLabel}
                </span>{' '}
                of memory.
              </p>
            )}
          </div>
          <button
            type="button"
            onClick={() => void run()}
            disabled={analyze.loading || busy}
            aria-label="Refresh"
            data-testid="perf-refresh"
            className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-line bg-surface-2 disabled:opacity-50"
          >
            <RefreshCw size={16} className={analyze.loading ? 'animate-spin' : ''} aria-hidden />
          </button>
        </div>
        <p className="m-0 mt-2 break-words text-xs text-muted" data-testid="perf-explain">
          Sleep mode stops an app from starting with your computer and asks it to close, so it stops using memory and battery.
          Nothing is deleted, and Wake puts its startup items back exactly as they were. Programs are never force-closed.
        </p>
        <button
          type="button"
          onClick={() => setPending({ kind: 'all', apps: canSleep })}
          disabled={busy || canSleep.length === 0}
          data-testid="perf-sleep-all"
          className="mt-3 flex h-10 w-full items-center justify-center gap-2 rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg disabled:opacity-50"
        >
          <Moon size={16} aria-hidden /> Sleep all ({canSleep.length})
        </button>
      </Card>

      {analyze.data?.stateError && (
        <p className="m-0 break-words rounded-xl border border-warn/40 bg-warn/10 p-2 text-xs" data-testid="perf-state-error">
          {analyze.data.stateError}
        </p>
      )}

      {note && (
        <p
          className={`m-0 break-words rounded-xl border p-2 text-xs ${note.ok ? 'border-ok/40 bg-ok/10' : 'border-warn/40 bg-warn/10'}`}
          data-testid="perf-note"
          role="status"
        >
          {note.text}
        </p>
      )}

      {sleep.loading && (
        <RunProgress label="Putting apps to sleep" progress={sleep.progress} onCancel={sleep.cancel} testId="perf-progress" />
      )}
      {wake.loading && <RunProgress label="Waking apps" progress={wake.progress} onCancel={wake.cancel} testId="perf-progress" />}

      {apps !== null && (
        <Card title={`Apps (${active.length})`} testId="perf-list" className="!p-0">
          {active.length === 0 ? (
            <div data-testid="perf-empty">
              <EmptyState title="Nothing to put to sleep" hint="No running apps or startup apps were found." />
            </div>
          ) : (
            <ul className="m-0 list-none divide-y divide-line p-0">
              {active.map((a) => (
                <li key={a.appId} data-testid={`perf-row-${a.appId}`} className="flex min-w-0 items-center gap-1 px-3 py-1.5">
                  <span className="min-w-0 flex-1">
                    <span className="flex min-w-0 items-center gap-1.5">
                      {a.protected && <Lock size={13} className="shrink-0 text-muted" aria-label="Protected" />}
                      <span className="min-w-0 break-words text-sm font-medium" data-testid="perf-name">
                        {a.name}
                      </span>
                    </span>
                    <span className="block break-words text-xs text-muted" data-testid="perf-sub">
                      {a.protected ? 'Protected: security or system software' : appSubline(a)}
                    </span>
                  </span>
                  <span className="shrink-0 text-[11px] text-muted">Sleep</span>
                  <Switch
                    checked={false}
                    disabled={a.protected || busy}
                    onChange={() => setPending({ kind: 'one', app: a })}
                    ariaLabel={`Sleep ${a.name}`}
                    testId={`perf-sleep-${a.appId}`}
                  />
                </li>
              ))}
            </ul>
          )}
        </Card>
      )}

      {asleep.length > 0 && (
        <Card title={`Sleeping (${asleep.length})`} testId="perf-sleeping" className="!p-0">
          <ul className="m-0 list-none divide-y divide-line p-0">
            {asleep.map((a) => (
              <li key={a.appId} data-testid={`perf-sleeping-${a.appId}`} className="flex min-w-0 items-center gap-2 px-3 py-1.5">
                <Moon size={14} className="shrink-0 text-muted" aria-hidden />
                <span className="min-w-0 flex-1 break-words text-sm font-medium">{a.name}</span>
                <button
                  type="button"
                  onClick={() => void doWake([a.appId])}
                  disabled={busy}
                  data-testid={`perf-wake-${a.appId}`}
                  className="flex h-9 shrink-0 items-center gap-1.5 rounded-xl border border-line bg-surface-2 px-3 text-sm font-medium disabled:opacity-50"
                >
                  <Sun size={14} aria-hidden /> Wake
                </button>
              </li>
            ))}
          </ul>
        </Card>
      )}

      <ConfirmSheet
        open={pending !== null}
        title={confirmCopy.title}
        message={confirmCopy.message}
        confirmLabel={confirmCopy.label}
        onConfirm={() => {
          if (!pending) return;
          void doSleep(pending.kind === 'one' ? [pending.app.appId] : pending.apps.map((a) => a.appId));
        }}
        onCancel={() => setPending(null)}
      />
    </div>
  );
}
