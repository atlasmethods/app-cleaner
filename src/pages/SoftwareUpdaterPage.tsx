import { useEffect, useMemo, useState } from 'react';
import { CheckCircle2, EyeOff, Loader2, MoreVertical, RefreshCw, ShieldAlert, XCircle } from 'lucide-react';
import type { UpdateEntry, UpdateReport } from '../api/software_updater';
import { BottomSheet } from '../components/BottomSheet';
import { Card } from '../components/Card';
import { Checkbox } from '../components/Checkbox';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { EmptyState } from '../components/EmptyState';
import { ErrorBanner } from '../components/ErrorBanner';
import { ProgressBar } from '../components/ProgressBar';
import { useCall } from '../hooks/useCall';
import {
  allUpdatable,
  effectiveSelection,
  resultById,
  selectable,
  toggleId,
  updateSourceLabel,
  versionChange,
} from '../lib/updates';
import { ApiCallError, call } from '../lib/transport';

type Confirm = 'selected' | 'all' | null;

export default function SoftwareUpdaterPage() {
  const list = useCall<UpdateEntry[], { refresh?: boolean }>('software_updater.list');
  const update = useCall<UpdateReport, { ids: string[] }>('software_updater.update');
  const updateAll = useCall<UpdateReport>('software_updater.update_all');
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  // Ignore toggles applied since the last load (the server list is refreshed lazily).
  const [ignoreEdits, setIgnoreEdits] = useState<Map<string, boolean>>(new Map());
  const [menu, setMenu] = useState<UpdateEntry | null>(null);
  const [confirm, setConfirm] = useState<Confirm>(null);
  const [report, setReport] = useState<UpdateReport | null>(null);
  const [actionError, setActionError] = useState<ApiCallError | null>(null);

  const load = list.run;
  useEffect(() => {
    void load();
  }, [load]);

  const entries: UpdateEntry[] | null = useMemo(
    () => list.data?.map((e) => ({ ...e, ignored: ignoreEdits.get(e.id) ?? e.ignored })) ?? null,
    [list.data, ignoreEdits],
  );
  const ready = entries ?? [];
  const ids = effectiveSelection(ready, chosen);
  const busy = list.loading || update.loading || updateAll.loading;
  const active = list.loading ? list : update.loading ? update : updateAll.loading ? updateAll : null;
  const results = useMemo(() => resultById(report?.results ?? []), [report]);
  const selectableCount = allUpdatable(ready);

  const refresh = async (withIndex: boolean) => {
    setReport(null);
    setActionError(null);
    setIgnoreEdits(new Map());
    await load(withIndex ? { refresh: true } : {});
  };

  const runUpdate = async (mode: 'selected' | 'all') => {
    setConfirm(null);
    setActionError(null);
    setReport(null);
    const r = mode === 'all' ? await updateAll.run() : await update.run({ ids });
    if (!r) return;
    setReport(r);
    setChosen(new Set());
    setIgnoreEdits(new Map());
    await load({});
  };

  const setIgnored = async (e: UpdateEntry, ignored: boolean) => {
    setMenu(null);
    setActionError(null);
    try {
      await call('software_updater.set_ignored', { id: e.id, ignored });
      setIgnoreEdits((m) => new Map(m).set(e.id, ignored));
      if (ignored) setChosen((s) => new Set([...s].filter((x) => x !== e.id)));
    } catch (err) {
      setActionError(ApiCallError.from(err));
    }
  };

  const allChecked = selectableCount > 0 && ids.length === selectableCount;
  const error = actionError ?? list.error ?? update.error ?? updateAll.error;

  return (
    <div data-testid="page-updater" className="flex min-h-full flex-col gap-3 p-4">
      <ErrorBanner error={error} onRetry={list.error ? () => void refresh(false) : undefined} />

      <div className="flex items-center justify-between gap-2">
        <p className="m-0 min-w-0 flex-1 text-xs text-muted" data-testid="updater-summary">
          {entries === null
            ? 'Checking for updates...'
            : entries.length === 0
              ? 'Everything is up to date.'
              : `${entries.length} ${entries.length === 1 ? 'update' : 'updates'} available`}
        </p>
        <button
          type="button"
          onClick={() => void refresh(true)}
          disabled={busy}
          data-testid="btn-refresh"
          className="flex h-10 shrink-0 items-center gap-1.5 rounded-xl border border-line bg-surface-2 px-3 text-sm font-medium disabled:opacity-50"
        >
          <RefreshCw size={15} className={list.loading ? 'animate-spin' : ''} aria-hidden /> Refresh
        </button>
      </div>

      {active && (
        <Card testId="updater-progress">
          <p className="m-0 mb-2 break-words text-xs text-muted" data-testid="updater-progress-message">
            {active.progress?.message ?? (active === list ? 'Checking for updates...' : 'Updating...')}
          </p>
          <ProgressBar value={(active.progress?.fraction ?? 0) * 100} label="Progress" />
          <button
            type="button"
            onClick={() => active.cancel()}
            data-testid="btn-cancel"
            className="mt-2 h-9 w-full rounded-xl border border-line bg-surface-2 text-sm font-medium"
          >
            Cancel
          </button>
        </Card>
      )}

      {report && !busy && (
        <Card
          title={`Update result: ${report.succeeded} succeeded${report.failed > 0 ? `, ${report.failed} failed` : ''}`}
          testId="updater-report"
          className="!p-0"
        >
          <ul className="m-0 list-none divide-y divide-line p-0">
            {report.results.map((r) => (
              <li key={r.id} className="flex min-w-0 items-start gap-2 px-3 py-1.5" data-testid={`report-${r.id}`}>
                {r.ok ? (
                  <CheckCircle2 size={16} className="mt-0.5 shrink-0 text-ok" aria-label="Updated" />
                ) : (
                  <XCircle size={16} className="mt-0.5 shrink-0 text-danger" aria-label="Failed" />
                )}
                <span className="min-w-0 flex-1">
                  <span className="block break-words text-sm">{r.name || r.id}</span>
                  <span className="block break-words text-xs text-muted">{r.message}</span>
                </span>
              </li>
            ))}
          </ul>
        </Card>
      )}

      {entries !== null && entries.length === 0 && !busy && (
        <EmptyState icon={CheckCircle2} title="Up to date" hint="No package manager reports an available update." />
      )}

      {entries !== null && entries.length > 0 && (
        <Card
          title={`Available updates (${entries.length})`}
          testId="updater-list"
          className="!p-0"
        >
          <div className="flex items-center gap-2 border-b border-line px-3 py-1.5 text-xs">
            <Checkbox
              checked={allChecked}
              indeterminate={ids.length > 0 && !allChecked}
              onChange={(v) => setChosen(v ? new Set(selectable(ready).map((e) => e.id)) : new Set())}
              ariaLabel="Select all"
              testId="updater-select-all"
              disabled={busy || selectableCount === 0}
            />
            <span className="text-muted">{ids.length} selected</span>
          </div>
          <ul className="m-0 list-none divide-y divide-line p-0">
            {entries.map((e) => {
              const res = results.get(e.id);
              return (
                <li
                  key={e.id}
                  data-testid={`update-${e.id}`}
                  className={`flex min-w-0 items-start gap-2 px-3 py-2 ${e.ignored ? 'opacity-60' : ''}`}
                >
                  <span className="mt-0.5">
                    <Checkbox
                      checked={ids.includes(e.id)}
                      disabled={e.ignored || busy}
                      onChange={() => setChosen((s) => toggleId(s, e.id))}
                      ariaLabel={`Select ${e.name}`}
                      testId={`update-select-${e.id}`}
                    />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5">
                      <span className="break-words text-sm font-medium" data-testid="update-name">
                        {e.name}
                      </span>
                      <span className="rounded bg-surface-2 px-1 text-[11px] text-muted" data-testid="update-source">
                        {updateSourceLabel(e.source)}
                      </span>
                      {e.security && (
                        <span
                          className="flex items-center gap-0.5 rounded bg-danger/15 px-1 text-[11px] font-medium text-danger"
                          data-testid="update-security"
                        >
                          <ShieldAlert size={11} aria-hidden /> Security
                        </span>
                      )}
                      {e.ignored && (
                        <span className="flex items-center gap-0.5 rounded bg-surface-2 px-1 text-[11px] text-muted" data-testid="update-ignored">
                          <EyeOff size={11} aria-hidden /> Ignored
                        </span>
                      )}
                    </span>
                    <span className="block break-all text-xs text-muted" data-testid="update-versions">
                      {versionChange(e)}
                    </span>
                    {res && (
                      <span
                        className={`block break-words text-xs ${res.ok ? 'text-ok' : 'text-danger'}`}
                        data-testid={`update-result-${e.id}`}
                      >
                        {res.ok ? 'Updated' : `Failed: ${res.message}`}
                      </span>
                    )}
                  </span>
                  <button
                    type="button"
                    onClick={() => setMenu(e)}
                    aria-label={`Options for ${e.name}`}
                    data-testid={`update-menu-${e.id}`}
                    className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full border-0 bg-transparent"
                  >
                    <MoreVertical size={16} aria-hidden />
                  </button>
                </li>
              );
            })}
          </ul>
        </Card>
      )}

      <div
        data-testid="action-bar"
        className="sticky bottom-0 z-10 mt-auto flex gap-2 rounded-xl border border-line bg-surface p-2 shadow-lg"
      >
        <button
          type="button"
          onClick={() => setConfirm('selected')}
          disabled={busy || ids.length === 0}
          data-testid="btn-update-selected"
          className="flex h-10 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border border-line bg-surface-2 px-2 text-sm font-semibold disabled:opacity-50"
        >
          {update.loading && <Loader2 size={16} className="animate-spin" aria-hidden />}
          Update selected ({ids.length})
        </button>
        <button
          type="button"
          onClick={() => setConfirm('all')}
          disabled={busy || selectableCount === 0}
          data-testid="btn-update-all"
          className="flex h-10 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border-0 bg-accent px-2 text-sm font-semibold text-accent-fg disabled:opacity-50"
        >
          {updateAll.loading && <Loader2 size={16} className="animate-spin" aria-hidden />}
          Update all
        </button>
      </div>

      <BottomSheet
        open={menu !== null}
        title={menu?.name ?? ''}
        subtitle={menu ? `${updateSourceLabel(menu.source)} - ${versionChange(menu)}` : undefined}
        onClose={() => setMenu(null)}
        testId="update-sheet"
      >
        {menu && (
          <button
            type="button"
            onClick={() => void setIgnored(menu, !menu.ignored)}
            data-testid="update-toggle-ignore"
            className="flex h-11 w-full items-center gap-2 rounded-xl border border-line bg-surface-2 px-3 text-left text-sm font-medium"
          >
            <EyeOff size={16} aria-hidden /> {menu.ignored ? 'Stop ignoring this update' : 'Ignore this update'}
          </button>
        )}
        <p className="m-0 text-xs text-muted">Ignored updates are skipped by &quot;Update all&quot;.</p>
      </BottomSheet>

      <ConfirmSheet
        open={confirm !== null}
        title={
          confirm === 'all'
            ? `Update all ${selectableCount} ${selectableCount === 1 ? 'program' : 'programs'}?`
            : `Update ${ids.length} selected ${ids.length === 1 ? 'program' : 'programs'}?`
        }
        message="Programs that are running may need a restart. You may be asked for your administrator password."
        confirmLabel="Update"
        onConfirm={() => void runUpdate(confirm === 'all' ? 'all' : 'selected')}
        onCancel={() => setConfirm(null)}
      />
    </div>
  );
}
