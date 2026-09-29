import { useCallback, useEffect, useMemo, useState } from 'react';
import { Loader2, RefreshCw, Search } from 'lucide-react';
import type { AppEntry, Leftover, RemoveLeftoversResult, RunResult } from '../api/uninstall';
import { Card } from '../components/Card';
import { Checkbox } from '../components/Checkbox';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { EmptyState } from '../components/EmptyState';
import { ErrorBanner } from '../components/ErrorBanner';
import { ProgressBar } from '../components/ProgressBar';
import { useCall } from '../hooks/useCall';
import { sourceLabel, subtitle, uninstallWarning, visibleApps, type SortKey } from '../lib/apps';
import { formatBytes } from '../lib/format';
import { ApiCallError, call } from '../lib/transport';
import { AppSheet } from './uninstall/AppSheet';
import { LeftoversCard } from './uninstall/LeftoversCard';

const PAGE = 150;

type Pending =
  | { kind: 'uninstall'; app: AppEntry }
  | { kind: 'force'; app: AppEntry; alsoRemoves: string[] }
  | { kind: 'repair'; app: AppEntry }
  | { kind: 'rename'; app: AppEntry; name: string }
  | { kind: 'removeEntry'; app: AppEntry };

interface LeftoverState {
  id: string;
  name: string;
  bundleId?: string;
  items: Leftover[];
}

export default function UninstallPage() {
  const list = useCall<AppEntry[]>('uninstall.list');
  const run = useCall<RunResult, { id: string; force?: boolean }>('uninstall.run');
  const repair = useCall<RunResult, { id: string }>('uninstall.repair');
  const removeEntry = useCall<RunResult, { id: string }>('uninstall.remove_entry');
  const rename = useCall<RunResult, { id: string; name: string }>('uninstall.rename_entry');
  const ops = [run, repair, removeEntry, rename];
  const active = ops.find((o) => o.loading) ?? null;

  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<SortKey>('name');
  const [showSystem, setShowSystem] = useState(false);
  const [limit, setLimit] = useState(PAGE);
  const [menuApp, setMenuApp] = useState<AppEntry | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
  const [leftovers, setLeftovers] = useState<LeftoverState | null>(null);
  const [leftoverSel, setLeftoverSel] = useState<Set<string>>(new Set());
  const [confirmLeftovers, setConfirmLeftovers] = useState(false);
  const [cleaning, setCleaning] = useState(false);
  const [actionError, setActionError] = useState<ApiCallError | null>(null);

  const loadList = list.run;
  useEffect(() => {
    void loadList();
  }, [loadList]);

  const apps = list.data;
  const shown = useMemo(
    () => (apps ? visibleApps(apps, { query, showSystem, sort }) : []),
    [apps, query, showSystem, sort],
  );
  const hiddenSystem = apps ? apps.filter((a) => a.isSystem).length : 0;

  const startLeftovers = (r: RunResult) => {
    if (r.ok && r.leftovers.length > 0) {
      setLeftovers({ id: r.id, name: r.name, bundleId: r.bundleId, items: r.leftovers });
      setLeftoverSel(new Set(r.leftovers.map((l) => l.path)));
    } else {
      setLeftovers(null);
    }
  };

  const describe = (r: RunResult): string => {
    const msg = r.message.replace(/[.\s]+$/, '');
    return `${r.name}: ${msg}.${r.ok && r.rebootRequired ? ' A restart is needed to finish.' : ''}`;
  };

  const doUninstall = async (app: AppEntry, force: boolean) => {
    setPending(null);
    setNote(null);
    setLeftovers(null);
    setActionError(null);
    const r = await run.run(force ? { id: app.id, force: true } : { id: app.id });
    if (!r) return;
    if (r.needsForce) {
      setPending({ kind: 'force', app, alsoRemoves: r.alsoRemoves });
      return;
    }
    setNote({ ok: r.ok, text: describe(r) });
    startLeftovers(r);
    if (r.ok) await loadList();
  };

  const doSimple = async (fn: () => Promise<RunResult | undefined>) => {
    setPending(null);
    setNote(null);
    setLeftovers(null);
    setActionError(null);
    const r = await fn();
    if (!r) return;
    setNote({
      ok: r.ok,
      text: describe(r) + (r.backupPath ? ` Backup: ${r.backupPath}` : ''),
    });
    if (r.ok) await loadList();
  };

  const confirm = () => {
    const p = pending;
    if (!p) return;
    switch (p.kind) {
      case 'uninstall':
        return void doUninstall(p.app, p.app.isSystem);
      case 'force':
        return void doUninstall(p.app, true);
      case 'repair':
        return void doSimple(() => repair.run({ id: p.app.id }));
      case 'rename':
        return void doSimple(() => rename.run({ id: p.app.id, name: p.name }));
      case 'removeEntry':
        return void doSimple(() => removeEntry.run({ id: p.app.id }));
    }
  };

  const removeLeftovers = useCallback(async () => {
    if (!leftovers) return;
    setConfirmLeftovers(false);
    setCleaning(true);
    setActionError(null);
    try {
      const paths = leftovers.items.filter((l) => leftoverSel.has(l.path)).map((l) => l.path);
      const res = await call<RemoveLeftoversResult>('uninstall.remove_leftovers', {
        name: leftovers.name,
        id: leftovers.id,
        ...(leftovers.bundleId ? { bundleId: leftovers.bundleId } : {}),
        paths,
      });
      const failed = res.results.filter((r) => !r.ok);
      setNote({
        ok: failed.length === 0,
        text:
          `Removed ${res.results.length - failed.length} of ${res.results.length} leftover items (${formatBytes(res.totalBytes)} freed).` +
          (failed.length > 0 ? ` ${failed.length} could not be removed: ${failed[0]?.error ?? ''}` : ''),
      });
      setLeftovers(null);
    } catch (e) {
      setActionError(ApiCallError.from(e));
    } finally {
      setCleaning(false);
    }
  }, [leftovers, leftoverSel]);

  const error = actionError ?? list.error ?? run.error ?? repair.error ?? removeEntry.error ?? rename.error;
  const busy = active !== null || cleaning;

  const confirmCopy = ((): { title: string; message: string; label: string; danger: boolean } => {
    if (!pending) return { title: '', message: '', label: 'Confirm', danger: false };
    const n = pending.app.name;
    switch (pending.kind) {
      case 'uninstall':
        return {
          title: `Uninstall ${n}?`,
          message: `${uninstallWarning(pending.app)} You may be asked for your administrator password.`,
          label: 'Uninstall',
          danger: true,
        };
      case 'force':
        return {
          title: `Also remove ${pending.alsoRemoves.length} other ${pending.alsoRemoves.length === 1 ? 'package' : 'packages'}?`,
          message: `Removing ${n} also removes: ${pending.alsoRemoves.slice(0, 12).join(', ')}${pending.alsoRemoves.length > 12 ? ', ...' : ''}.`,
          label: 'Remove all',
          danger: true,
        };
      case 'repair':
        return {
          title: `Repair ${n}?`,
          message: 'The installer will run and may reinstall program files. You may be asked for permission.',
          label: 'Repair',
          danger: false,
        };
      case 'rename':
        return {
          title: 'Rename entry?',
          message: `The entry "${n}" will show as "${pending.name}" in the programs list. The program itself is not changed. A registry backup is saved first.`,
          label: 'Rename',
          danger: false,
        };
      case 'removeEntry':
        return {
          title: `Delete the entry for ${n}?`,
          message: `This removes only the list entry (registry key), not the program's files. A backup of the key is saved first.`,
          label: 'Delete entry',
          danger: true,
        };
    }
  })();

  return (
    <div data-testid="page-uninstall" className="flex min-h-full flex-col gap-3 p-4">
      <ErrorBanner error={error} onRetry={list.error ? () => void loadList() : undefined} />

      <div className="flex gap-2">
        <label className="relative min-w-0 flex-1">
          <Search size={15} className="pointer-events-none absolute left-2.5 top-3 text-muted" aria-hidden />
          <input
            type="search"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setLimit(PAGE);
            }}
            placeholder="Search programs"
            aria-label="Search programs"
            data-testid="uninstall-search"
            className="h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 pl-8 pr-2 text-sm"
          />
        </label>
        <button
          type="button"
          onClick={() => void loadList()}
          disabled={list.loading || busy}
          aria-label="Reload list"
          data-testid="uninstall-reload"
          className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-line bg-surface-2 disabled:opacity-50"
        >
          <RefreshCw size={16} className={list.loading ? 'animate-spin' : ''} aria-hidden />
        </button>
      </div>

      <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-1 text-xs">
        <label className="flex items-center gap-1.5">
          <span className="text-muted">Sort</span>
          <select
            value={sort}
            onChange={(e) => setSort(e.target.value as SortKey)}
            data-testid="uninstall-sort"
            aria-label="Sort by"
            className="h-8 rounded-lg border border-line bg-surface-2 px-1.5 text-xs"
          >
            <option value="name">Name</option>
            <option value="size">Size</option>
            <option value="date">Install date</option>
          </select>
        </label>
        <label className="flex items-center gap-1.5">
          <Checkbox
            checked={showSystem}
            onChange={(v) => {
              setShowSystem(v);
              setLimit(PAGE);
            }}
            ariaLabel="Show system components"
            testId="uninstall-show-system"
          />
          <span>Show system components{hiddenSystem > 0 ? ` (${hiddenSystem})` : ''}</span>
        </label>
      </div>

      {note && (
        <p
          className={`m-0 break-words rounded-xl border p-2 text-xs ${note.ok ? 'border-ok/40 bg-ok/10' : 'border-danger/40 bg-danger/10'}`}
          data-testid="uninstall-note"
          role="status"
        >
          {note.text}
        </p>
      )}

      {active && (
        <Card testId="uninstall-progress">
          <p className="m-0 mb-2 break-words text-xs text-muted" data-testid="uninstall-progress-message">
            {active.progress?.message ?? 'Working...'}
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

      {leftovers && !busy && (
        <LeftoversCard
          appName={leftovers.name}
          items={leftovers.items}
          selected={leftoverSel}
          busy={cleaning}
          onToggle={(p) =>
            setLeftoverSel((s) => {
              const n = new Set(s);
              if (n.has(p)) n.delete(p);
              else n.add(p);
              return n;
            })
          }
          onRemove={() => setConfirmLeftovers(true)}
          onDismiss={() => setLeftovers(null)}
        />
      )}
      {cleaning && (
        <p className="m-0 flex items-center gap-2 text-xs text-muted" data-testid="leftovers-cleaning">
          <Loader2 size={14} className="animate-spin" aria-hidden /> Removing leftovers...
        </p>
      )}

      {apps === null && !list.error && (
        <p className="py-8 text-center text-sm text-muted" data-testid="uninstall-loading">
          Reading installed programs...
        </p>
      )}

      {apps !== null && (
        <Card title={`Installed programs (${shown.length})`} testId="uninstall-list" className="!p-0">
          {shown.length === 0 ? (
            <div data-testid="uninstall-empty">
              <EmptyState
                title={apps.length === 0 ? 'No programs found' : 'Nothing matches'}
                hint={apps.length === 0 ? 'No supported package manager reported any installed program.' : 'Try a different search or show system components.'}
              />
            </div>
          ) : (
            <ul className="m-0 list-none divide-y divide-line p-0">
              {shown.slice(0, limit).map((a) => (
                <li key={a.id}>
                  <button
                    type="button"
                    onClick={() => setMenuApp(a)}
                    disabled={busy}
                    data-testid={`app-row-${a.id}`}
                    className="flex min-h-12 w-full min-w-0 items-center gap-2 border-0 bg-transparent px-3 py-1.5 text-left disabled:opacity-60"
                  >
                    <span className="min-w-0 flex-1">
                      <span className="block break-words text-sm font-medium" data-testid="app-name">
                        {a.name}
                      </span>
                      <span className="block break-words text-xs text-muted">{subtitle(a) || ' '}</span>
                    </span>
                    <span className="flex shrink-0 flex-col items-end gap-0.5 text-[11px] text-muted">
                      <span data-testid="app-size">{a.sizeBytes !== undefined ? formatBytes(a.sizeBytes) : ''}</span>
                      <span className="rounded bg-surface-2 px-1">{sourceLabel(a.source)}</span>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          {shown.length > limit && (
            <div className="border-t border-line p-2">
              <button
                type="button"
                onClick={() => setLimit((l) => l + 300)}
                data-testid="uninstall-more"
                className="h-9 w-full rounded-xl border border-line bg-surface-2 text-sm"
              >
                Show more ({shown.length - limit})
              </button>
            </div>
          )}
        </Card>
      )}

      <AppSheet
        key={menuApp?.id ?? 'none'}
        app={menuApp}
        onClose={() => setMenuApp(null)}
        onUninstall={(app) => {
          setMenuApp(null);
          setPending({ kind: 'uninstall', app });
        }}
        onRepair={(app) => {
          setMenuApp(null);
          setPending({ kind: 'repair', app });
        }}
        onRename={(app, name) => {
          setMenuApp(null);
          setPending({ kind: 'rename', app, name });
        }}
        onRemoveEntry={(app) => {
          setMenuApp(null);
          setPending({ kind: 'removeEntry', app });
        }}
      />

      <ConfirmSheet
        open={pending !== null}
        title={confirmCopy.title}
        message={confirmCopy.message}
        confirmLabel={confirmCopy.label}
        danger={confirmCopy.danger}
        onConfirm={confirm}
        onCancel={() => setPending(null)}
      />

      <ConfirmSheet
        open={confirmLeftovers}
        title="Remove leftovers?"
        message={`${leftoverSel.size} selected ${leftoverSel.size === 1 ? 'item' : 'items'} will be permanently deleted. This cannot be undone.`}
        confirmLabel="Remove"
        danger
        onConfirm={() => void removeLeftovers()}
        onCancel={() => setConfirmLeftovers(false)}
      />
    </div>
  );
}
