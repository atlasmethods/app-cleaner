import { CalendarClock, Pencil, Play, Plus, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { useCall } from '../hooks/useCall';
import type { Backend, RemoveResult, Schedule } from '../api/scheduler';
import { Card } from '../components/Card';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { EmptyState } from '../components/EmptyState';
import { ErrorBanner } from '../components/ErrorBanner';
import { Switch } from '../components/Switch';
import { formatBytes, formatWhen } from '../lib/format';
import { describeSchedule } from '../lib/schedule';
import { ApiCallError, call } from '../lib/transport';
import { ScheduleSheet } from './schedules/ScheduleSheet';

function lastRunLine(s: Schedule): string {
  if (!s.lastRun) return 'Has not run yet';
  const when = formatWhen(s.lastRun);
  const r = s.lastResult;
  if (!r) return `Last run ${when}`;
  if (!r.ok) return `Last run ${when}: ${r.message ?? 'failed'}`;
  const note = r.message ? ` (${r.message})` : '';
  return `Last run ${when}: cleaned ${formatBytes(r.totalBytes)}${note}`;
}

export default function SchedulesPage() {
  const [schedules, setSchedules] = useState<Schedule[] | null>(null);
  const [backend, setBackend] = useState<Backend | null>(null);
  const [error, setError] = useState<ApiCallError | null>(null);
  const [sheet, setSheet] = useState<{ editing: Schedule | null } | null>(null);
  const [deleting, setDeleting] = useState<Schedule | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const list = useCall<Schedule[]>('scheduler.list');
  const run = list.run;
  const load = useCallback(async () => {
    const r = await run();
    if (r) setSchedules(r);
  }, [run]);

  useEffect(() => {
    void run().then((r) => {
      if (r) setSchedules(r);
    });
    call<Backend>('scheduler.backend')
      .then(setBackend)
      .catch(() => undefined);
  }, [run]);

  const replace = (s: Schedule) => setSchedules((all) => (all ? all.map((x) => (x.id === s.id ? s : x)) : all));

  const toggle = async (s: Schedule, enabled: boolean) => {
    setBusyId(s.id);
    setNote(null);
    try {
      replace(await call<Schedule>('scheduler.set_enabled', { id: s.id, enabled }));
      setError(null);
    } catch (e) {
      setError(ApiCallError.from(e));
    } finally {
      setBusyId(null);
    }
  };

  const runNow = async (s: Schedule) => {
    setBusyId(s.id);
    setNote(null);
    try {
      const r = await call<{ schedule: Schedule; report: { totalBytes: number } }>('scheduler.run_now', { id: s.id });
      replace(r.schedule);
      setNote(`${s.name}: cleaned ${formatBytes(r.report.totalBytes)}`);
      setError(null);
    } catch (e) {
      setError(ApiCallError.from(e));
      void load(); // the failed run is recorded in the list
    } finally {
      setBusyId(null);
    }
  };

  const remove = async (s: Schedule) => {
    setDeleting(null);
    setBusyId(s.id);
    setNote(null);
    try {
      const r = await call<RemoveResult>('scheduler.remove', { id: s.id });
      setSchedules((all) => (all ? all.filter((x) => x.id !== s.id) : all));
      if (r.warnings.length > 0) setNote(`Removed, but the system job could not be fully cleaned up: ${r.warnings.join('; ')}`);
      setError(null);
    } catch (e) {
      setError(ApiCallError.from(e));
    } finally {
      setBusyId(null);
    }
  };

  const saved = (s: Schedule) => {
    setSheet(null);
    setNote(null);
    setSchedules((all) => {
      if (!all) return [s];
      return all.some((x) => x.id === s.id) ? all.map((x) => (x.id === s.id ? s : x)) : [...all, s];
    });
  };

  return (
    <div data-testid="page-schedules" className="flex flex-col gap-3 p-4">
      <ErrorBanner error={error ?? list.error} onRetry={() => void load()} />

      {backend && (
        <p
          data-testid="schedules-backend"
          className={`m-0 break-words rounded-xl border p-3 text-xs ${
            backend.available ? 'border-line bg-surface text-muted' : 'border-warn/40 bg-warn/10'
          }`}
        >
          {backend.detail}
        </p>
      )}

      {note && (
        <p role="status" data-testid="schedules-note" className="m-0 break-words rounded-xl border border-ok/40 bg-ok/10 p-3 text-sm">
          {note}
        </p>
      )}

      <button
        type="button"
        onClick={() => setSheet({ editing: null })}
        data-testid="schedule-add"
        className="flex h-11 items-center justify-center gap-2 rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg"
      >
        <Plus size={18} aria-hidden /> Add schedule
      </button>

      {schedules === null && !error && !list.error && <p className="py-6 text-center text-sm text-muted">Loading schedules...</p>}
      {schedules?.length === 0 && (
        <EmptyState icon={CalendarClock} title="No schedules yet" hint="Add one to clean automatically." />
      )}

      {schedules?.map((s) => (
        <Card key={s.id} testId={`schedule-${s.id}`}>
          <div className="flex min-w-0 items-start gap-2">
            <div className="min-w-0 flex-1">
              <p className="m-0 break-words text-sm font-semibold" data-testid={`schedule-name-${s.id}`}>
                {s.name}
              </p>
              <p className="m-0 break-words text-xs text-muted" data-testid={`schedule-when-${s.id}`}>
                {describeSchedule(s)}
              </p>
              <p className="m-0 mt-1 break-words text-xs text-muted" data-testid={`schedule-last-${s.id}`}>
                {lastRunLine(s)}
              </p>
            </div>
            <Switch
              checked={s.enabled}
              disabled={busyId === s.id}
              onChange={(on) => void toggle(s, on)}
              ariaLabel={`${s.name} enabled`}
              testId={`schedule-toggle-${s.id}`}
            />
          </div>
          <div className="mt-2 flex flex-wrap gap-2">
            <button
              type="button"
              disabled={busyId === s.id}
              onClick={() => void runNow(s)}
              data-testid={`schedule-run-${s.id}`}
              className="flex h-9 items-center gap-1.5 rounded-xl border border-line bg-surface-2 px-3 text-xs font-medium disabled:opacity-60"
            >
              <Play size={14} aria-hidden /> {busyId === s.id ? 'Working...' : 'Run now'}
            </button>
            <button
              type="button"
              onClick={() => setSheet({ editing: s })}
              data-testid={`schedule-edit-${s.id}`}
              className="flex h-9 items-center gap-1.5 rounded-xl border border-line bg-surface-2 px-3 text-xs font-medium"
            >
              <Pencil size={14} aria-hidden /> Edit
            </button>
            <button
              type="button"
              onClick={() => setDeleting(s)}
              data-testid={`schedule-delete-${s.id}`}
              className="flex h-9 items-center gap-1.5 rounded-xl border border-line bg-surface-2 px-3 text-xs font-medium text-danger"
            >
              <Trash2 size={14} aria-hidden /> Delete
            </button>
          </div>
        </Card>
      ))}

      {sheet && (
        <ScheduleSheet
          key={sheet.editing?.id ?? 'new'}
          editing={sheet.editing}
          others={schedules ?? []}
          onClose={() => setSheet(null)}
          onSaved={saved}
        />
      )}

      <ConfirmSheet
        open={deleting !== null}
        danger
        title={`Delete "${deleting?.name ?? ''}"?`}
        message="It will no longer run. Anything it already cleaned stays cleaned."
        confirmLabel="Delete"
        onCancel={() => setDeleting(null)}
        onConfirm={() => deleting && void remove(deleting)}
      />
    </div>
  );
}
