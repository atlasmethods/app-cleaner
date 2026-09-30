import { useCallback, useEffect, useMemo, useState } from 'react';
import { EllipsisVertical, Lock, RefreshCw, Search, Trash2 } from 'lucide-react';
import type { RemoveResult, SetEnabledResult, StartupItem, StartupKind } from '../api/startup';
import { BottomSheet } from '../components/BottomSheet';
import { Card } from '../components/Card';
import { Checkbox } from '../components/Checkbox';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { EmptyState } from '../components/EmptyState';
import { ErrorBanner } from '../components/ErrorBanner';
import { Switch } from '../components/Switch';
import { useCall } from '../hooks/useCall';
import { IMPACT_LABEL, KIND_LABEL, deleteMessage, hiddenSystemCount, kindsPresent, subline, visibleItems } from '../lib/startup';
import { ApiCallError, call } from '../lib/transport';

type Pending =
  | { kind: 'delete'; item: StartupItem }
  | { kind: 'toggle'; item: StartupItem; enabled: boolean };

const IMPACT_STYLE: Record<StartupItem['impact'], string> = {
  high: 'border-danger/40 bg-danger/10 text-danger',
  medium: 'border-warn/40 bg-warn/10',
  low: 'border-ok/40 bg-ok/10',
  unknown: 'border-line bg-surface-2 text-muted',
};

export default function StartupPage() {
  const list = useCall<StartupItem[]>('startup.list');
  const [items, setItems] = useState<StartupItem[] | null>(null);
  const [query, setQuery] = useState('');
  const [kind, setKind] = useState<StartupKind | 'all'>('all');
  const [showSystem, setShowSystem] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [menu, setMenu] = useState<StartupItem | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
  const [actionError, setActionError] = useState<ApiCallError | null>(null);

  const load = list.run;
  useEffect(() => {
    void load().then((r) => {
      if (r) setItems(r);
    });
  }, [load]);

  const reload = useCallback(async () => {
    const r = await load();
    if (r) setItems(r);
  }, [load]);

  const kinds = useMemo(() => (items ? kindsPresent(items, showSystem) : []), [items, showSystem]);
  // A chip that disappeared (for example after deleting its last item) falls back to All.
  const activeKind = kind === 'all' || kinds.some((k) => k.kind === kind) ? kind : 'all';
  const shown = useMemo(
    () => (items ? visibleItems(items, { query, kind: activeKind, showSystem }) : []),
    [items, query, activeKind, showSystem],
  );
  const hidden = items ? hiddenSystemCount(items) : 0;
  const total = items ? kinds.reduce((s, k) => s + k.count, 0) : 0;

  const applyToggle = async (item: StartupItem, enabled: boolean) => {
    setPending(null);
    setNote(null);
    setActionError(null);
    setBusyId(item.id);
    try {
      const r = await call<SetEnabledResult>('startup.set_enabled', { id: item.id, enabled });
      setItems((cur) => cur?.map((i) => (i.id === item.id ? r.item : i)) ?? cur);
      setNote({ ok: true, text: `${item.name} is now ${r.item.enabled ? 'on' : 'off'} at startup.` });
    } catch (e) {
      setActionError(ApiCallError.from(e));
    } finally {
      setBusyId(null);
    }
  };

  const requestToggle = (item: StartupItem, enabled: boolean) => {
    // Machine-wide items and ones with a warning are confirmed; personal ones flip at once.
    if (item.scope === 'system' || item.warning) setPending({ kind: 'toggle', item, enabled });
    else void applyToggle(item, enabled);
  };

  const applyDelete = async (item: StartupItem) => {
    setPending(null);
    setNote(null);
    setActionError(null);
    setBusyId(item.id);
    try {
      const r = await call<RemoveResult>('startup.remove', { id: item.id });
      setItems((cur) => cur?.filter((i) => i.id !== item.id) ?? cur);
      setNote({ ok: true, text: `${item.name} was deleted. Backup saved as ${r.backupId}.` });
    } catch (e) {
      setActionError(ApiCallError.from(e));
    } finally {
      setBusyId(null);
    }
  };

  const confirm = () => {
    if (!pending) return;
    if (pending.kind === 'delete') void applyDelete(pending.item);
    else void applyToggle(pending.item, pending.enabled);
  };

  const copy = (() => {
    if (!pending) return { title: '', message: '', label: 'Confirm', danger: false };
    if (pending.kind === 'delete')
      return {
        title: `Delete ${pending.item.name}?`,
        message: deleteMessage(pending.item),
        label: 'Delete',
        danger: true,
      };
    const on = pending.enabled;
    return {
      title: `${on ? 'Turn on' : 'Turn off'} ${pending.item.name}?`,
      message:
        pending.item.warning ??
        (pending.item.scope === 'system'
          ? `This changes a setting for every user on this computer. You may be asked for your administrator password.`
          : ''),
      label: on ? 'Turn on' : 'Turn off',
      danger: !on,
    };
  })();

  const error = actionError ?? list.error;

  return (
    <div data-testid="page-startup" className="flex min-h-full flex-col gap-3 p-4">
      <ErrorBanner error={error} onRetry={list.error ? () => void reload() : undefined} />

      <div className="flex gap-2">
        <label className="relative min-w-0 flex-1">
          <Search size={15} className="pointer-events-none absolute left-2.5 top-3 text-muted" aria-hidden />
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search startup items"
            aria-label="Search startup items"
            data-testid="startup-search"
            className="h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 pl-8 pr-2 text-sm"
          />
        </label>
        <button
          type="button"
          onClick={() => void reload()}
          disabled={list.loading || busyId !== null}
          aria-label="Reload list"
          data-testid="startup-reload"
          className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-line bg-surface-2 disabled:opacity-50"
        >
          <RefreshCw size={16} className={list.loading ? 'animate-spin' : ''} aria-hidden />
        </button>
      </div>

      {kinds.length > 0 && (
        <div className="flex flex-wrap gap-1.5" role="group" aria-label="Filter by type" data-testid="startup-chips">
          {[{ kind: 'all' as const, label: 'All', count: total }, ...kinds].map((k) => (
            <button
              key={k.kind}
              type="button"
              onClick={() => setKind(k.kind)}
              aria-pressed={activeKind === k.kind}
              data-testid={`chip-${k.kind}`}
              className={`h-8 rounded-full border px-3 text-xs font-medium ${
                activeKind === k.kind ? 'border-accent bg-accent text-accent-fg' : 'border-line bg-surface-2'
              }`}
            >
              {k.label} ({k.count})
            </button>
          ))}
        </div>
      )}

      <label className="flex items-center gap-1.5 text-xs">
        <Checkbox
          checked={showSystem}
          onChange={setShowSystem}
          ariaLabel="Show system items"
          testId="startup-show-system"
        />
        <span>Show system items{hidden > 0 ? ` (${hidden})` : ''}</span>
      </label>

      {note && (
        <p
          className={`m-0 break-words rounded-xl border p-2 text-xs ${note.ok ? 'border-ok/40 bg-ok/10' : 'border-danger/40 bg-danger/10'}`}
          data-testid="startup-note"
          role="status"
        >
          {note.text}
        </p>
      )}

      {items === null && !list.error && (
        <p className="py-8 text-center text-sm text-muted" data-testid="startup-loading">
          {list.progress?.message ?? 'Reading startup items...'}
        </p>
      )}

      {items !== null && (
        <Card title={`Startup items (${shown.length})`} testId="startup-list" className="!p-0">
          {shown.length === 0 ? (
            <div data-testid="startup-empty">
              <EmptyState
                title={items.length === 0 ? 'Nothing starts automatically' : 'Nothing matches'}
                hint={items.length === 0 ? 'No startup items were found on this computer.' : 'Try another search or filter.'}
              />
            </div>
          ) : (
            <ul className="m-0 list-none divide-y divide-line p-0">
              {shown.map((i) => (
                <li
                  key={i.id}
                  data-testid={`startup-row-${i.id}`}
                  className={`flex min-w-0 items-center gap-1 px-3 py-1.5 ${i.critical ? 'opacity-60' : ''}`}
                >
                  <span className="min-w-0 flex-1">
                    <span className="flex min-w-0 items-center gap-1.5">
                      {i.critical && <Lock size={13} className="shrink-0 text-muted" aria-label="System item" />}
                      <span className="min-w-0 break-words text-sm font-medium" data-testid="startup-name">
                        {i.name}
                      </span>
                    </span>
                    <span className="block break-words text-xs text-muted">{subline(i)}</span>
                    {i.warning && !i.critical && (
                      <span className="mt-0.5 block break-words text-[11px] text-warn" data-testid="startup-warning">
                        {i.warning}
                      </span>
                    )}
                  </span>
                  <span
                    className={`shrink-0 rounded border px-1 text-[10px] ${IMPACT_STYLE[i.impact]}`}
                    data-testid="startup-impact"
                    title="How much memory and CPU it uses while running"
                  >
                    {IMPACT_LABEL[i.impact]}
                  </span>
                  <Switch
                    checked={i.enabled}
                    disabled={i.critical || !i.canDisable || busyId !== null}
                    onChange={(v) => requestToggle(i, v)}
                    ariaLabel={`${i.name} at startup`}
                    testId={`startup-toggle-${i.id}`}
                  />
                  <button
                    type="button"
                    onClick={() => setMenu(i)}
                    disabled={busyId !== null}
                    aria-label={`More for ${i.name}`}
                    data-testid={`startup-menu-${i.id}`}
                    className="flex h-11 w-9 shrink-0 items-center justify-center border-0 bg-transparent p-0 disabled:opacity-50"
                  >
                    <EllipsisVertical size={18} aria-hidden />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </Card>
      )}

      <BottomSheet
        open={menu !== null}
        title={menu?.name ?? ''}
        subtitle={menu ? `${KIND_LABEL[menu.kind]} - ${menu.scope === 'system' ? 'all users' : 'this user'}` : undefined}
        onClose={() => setMenu(null)}
        testId="startup-sheet"
      >
        {menu && (
          <>
            <dl className="m-0 grid min-w-0 grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
              <dt className="text-muted">Command</dt>
              <dd className="m-0 min-w-0 break-all" data-testid="startup-sheet-command">
                {menu.command || '-'}
              </dd>
              <dt className="text-muted">Location</dt>
              <dd className="m-0 min-w-0 break-all">{menu.location || '-'}</dd>
            </dl>
            {menu.critical && (
              <p className="m-0 text-xs text-muted">This item is needed by the system and cannot be changed.</p>
            )}
            {menu.canDelete && !menu.critical ? (
              <button
                type="button"
                onClick={() => {
                  const item = menu;
                  setMenu(null);
                  setPending({ kind: 'delete', item });
                }}
                data-testid="startup-action-delete"
                className="flex h-11 w-full items-center gap-2 rounded-xl border-0 bg-danger px-3 text-left text-sm font-medium text-white"
              >
                <Trash2 size={16} aria-hidden /> Delete
              </button>
            ) : (
              !menu.critical && <p className="m-0 text-xs text-muted">This kind of item can only be switched off, not deleted.</p>
            )}
          </>
        )}
      </BottomSheet>

      <ConfirmSheet
        open={pending !== null}
        title={copy.title}
        message={copy.message}
        confirmLabel={copy.label}
        danger={copy.danger}
        onConfirm={confirm}
        onCancel={() => setPending(null)}
      />
    </div>
  );
}
