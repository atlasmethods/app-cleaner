import { useCallback, useEffect, useMemo, useState } from 'react';
import { Lock, RefreshCw, RotateCcw, Search, Trash2 } from 'lucide-react';
import type { Plugin, RemovePluginResult, RestorePluginResult, SetPluginResult } from '../api/browser_plugins';
import { Card } from '../components/Card';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { EmptyState } from '../components/EmptyState';
import { ErrorBanner } from '../components/ErrorBanner';
import { Switch } from '../components/Switch';
import { useCall } from '../hooks/useCall';
import { TYPE_LABEL, closeBrowserName, filterPlugins, groupPlugins, profileTitle } from '../lib/plugins';
import { ApiCallError, call } from '../lib/transport';

export default function BrowserPluginsPage() {
  const list = useCall<Plugin[]>('browser_plugins.list');
  const [plugins, setPlugins] = useState<Plugin[] | null>(null);
  const [query, setQuery] = useState('');
  const [busyId, setBusyId] = useState<string | null>(null);
  const [removing, setRemoving] = useState<Plugin | null>(null);
  const [note, setNote] = useState<{ ok: boolean; text: string; undo?: { backupId: string; name: string } } | null>(null);
  const [closeFirst, setCloseFirst] = useState<string | null>(null);
  const [actionError, setActionError] = useState<ApiCallError | null>(null);

  const load = list.run;
  const reload = useCallback(async () => {
    const r = await load();
    if (r) setPlugins(r);
  }, [load]);
  useEffect(() => {
    void load().then((r) => {
      if (r) setPlugins(r);
    });
  }, [load]);

  const groups = useMemo(() => groupPlugins(plugins ? filterPlugins(plugins, query) : []), [plugins, query]);

  const fail = (e: unknown) => {
    const err = ApiCallError.from(e);
    const browser = closeBrowserName(err.message);
    if (browser) setCloseFirst(browser);
    else setActionError(err);
  };

  const toggle = async (p: Plugin, enabled: boolean) => {
    setNote(null);
    setActionError(null);
    setCloseFirst(null);
    setBusyId(p.id);
    try {
      const r = await call<SetPluginResult>('browser_plugins.set_enabled', { id: p.id, enabled });
      setPlugins((cur) => cur?.map((x) => (x.id === p.id ? r.plugin : x)) ?? cur);
      setNote({ ok: true, text: `${p.name} is now ${r.plugin.enabled ? 'on' : 'off'}.` });
    } catch (e) {
      fail(e);
    } finally {
      setBusyId(null);
    }
  };

  const remove = async (p: Plugin) => {
    setRemoving(null);
    setNote(null);
    setActionError(null);
    setCloseFirst(null);
    setBusyId(p.id);
    try {
      const r = await call<RemovePluginResult>('browser_plugins.remove', { id: p.id });
      setPlugins((cur) => cur?.filter((x) => x.id !== p.id) ?? cur);
      setNote({
        ok: true,
        text: `${p.name} was removed. Backup saved as ${r.backupId}.${r.note ? ` ${r.note}` : ''}`,
        undo: { backupId: r.backupId, name: p.name },
      });
    } catch (e) {
      fail(e);
    } finally {
      setBusyId(null);
    }
  };

  /** Put a just-removed add-on back from its backup (the browser must still be closed). */
  const undoRemove = async (backupId: string, name: string) => {
    setNote(null);
    setActionError(null);
    setCloseFirst(null);
    setBusyId(backupId);
    try {
      const r = await call<RestorePluginResult>('browser_plugins.restore_backup', { id: backupId });
      await reload();
      setNote({ ok: true, text: `${name} is back.${r.notes.length ? ` ${r.notes.join(' ')}` : ''}` });
    } catch (e) {
      // Kept so it can be tried again, for example after closing the browser.
      setNote({ ok: true, text: `${name} was removed. Backup saved as ${backupId}.`, undo: { backupId, name } });
      fail(e);
    } finally {
      setBusyId(null);
    }
  };

  const error = actionError ?? list.error;

  return (
    <div data-testid="page-plugins" className="flex min-h-full flex-col gap-3 p-4">
      <ErrorBanner error={error} onRetry={list.error ? () => void reload() : undefined} />

      {closeFirst && (
        <p
          role="alert"
          data-testid="plugins-close-notice"
          className="m-0 break-words rounded-xl border border-warn/40 bg-warn/10 p-2 text-sm"
        >
          Close {closeFirst} first, then try again. While it is open it would undo the change.
        </p>
      )}

      <div className="flex gap-2">
        <label className="relative min-w-0 flex-1">
          <Search size={15} className="pointer-events-none absolute left-2.5 top-3 text-muted" aria-hidden />
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search add-ons"
            aria-label="Search add-ons"
            data-testid="plugins-search"
            className="h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 pl-8 pr-2 text-sm"
          />
        </label>
        <button
          type="button"
          onClick={() => void reload()}
          disabled={list.loading || busyId !== null}
          aria-label="Reload list"
          data-testid="plugins-reload"
          className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-line bg-surface-2 disabled:opacity-50"
        >
          <RefreshCw size={16} className={list.loading ? 'animate-spin' : ''} aria-hidden />
        </button>
      </div>

      {note && (
        <div
          className={`flex min-w-0 flex-col gap-2 rounded-xl border p-2 text-xs ${note.ok ? 'border-ok/40 bg-ok/10' : 'border-danger/40 bg-danger/10'}`}
        >
          <p className="m-0 break-words" data-testid="plugins-note" role="status">
            {note.text}
          </p>
          {note.undo && (
            <button
              type="button"
              onClick={() => note.undo && void undoRemove(note.undo.backupId, note.undo.name)}
              disabled={busyId !== null}
              data-testid="plugins-undo"
              className="flex h-11 w-full items-center justify-center gap-1.5 rounded-xl border border-line bg-surface-2 text-sm font-medium disabled:opacity-50"
            >
              <RotateCcw size={14} aria-hidden /> Undo
            </button>
          )}
        </div>
      )}

      {plugins === null && !list.error && (
        <p className="py-8 text-center text-sm text-muted" data-testid="plugins-loading">
          Reading browser add-ons...
        </p>
      )}

      {plugins !== null && groups.length === 0 && (
        <div data-testid="plugins-empty">
          <EmptyState
            title={plugins.length === 0 ? 'No add-ons found' : 'Nothing matches'}
            hint={plugins.length === 0 ? 'No supported browser profile with add-ons was found.' : 'Try a different search.'}
          />
        </div>
      )}

      {groups.map((b) => (
        <section key={b.browser} data-testid={`plugins-browser-${b.browser}`} className="min-w-0">
          <h2 className="m-0 mb-1 flex flex-wrap items-baseline gap-x-2 text-sm font-semibold">
            {b.browserLabel}
            {b.running && (
              <span className="text-[11px] font-normal text-warn" data-testid={`plugins-running-${b.browser}`}>
                Running: close it to make changes
              </span>
            )}
          </h2>
          {b.profiles.map((pr) => (
            <Card
              key={pr.profile}
              title={profileTitle(pr)}
              testId={`plugins-profile-${b.browser}-${pr.profile}`}
              className="mb-2 !p-0 [&>h2]:px-3 [&>h2]:pt-3"
            >
              <ul className="m-0 list-none divide-y divide-line p-0">
                {pr.plugins.map((p) => (
                  <li key={p.id} data-testid={`plugin-row-${p.id}`} className="flex min-w-0 items-start gap-1 px-3 py-1.5">
                    <span className="min-w-0 flex-1">
                      <span className="flex min-w-0 flex-wrap items-center gap-x-1.5">
                        <span className="min-w-0 break-words text-sm font-medium" data-testid="plugin-name">
                          {p.name}
                        </span>
                        <span className="rounded bg-surface-2 px-1 text-[10px] text-muted" data-testid="plugin-type">
                          {TYPE_LABEL[p.type]}
                        </span>
                      </span>
                      <span className="block break-words text-xs text-muted">
                        {p.version ? `Version ${p.version}` : ''}
                        {p.version && p.description ? ' - ' : ''}
                        {p.description}
                      </span>
                      {p.note && !p.canDisable && (
                        <span className="mt-0.5 flex items-start gap-1 break-words text-[11px] text-warn" data-testid="plugin-note">
                          <Lock size={11} className="mt-0.5 shrink-0" aria-hidden /> {p.note}
                        </span>
                      )}
                    </span>
                    <Switch
                      checked={p.enabled}
                      disabled={!p.canDisable || busyId !== null}
                      onChange={(v) => void toggle(p, v)}
                      ariaLabel={`${p.name} enabled`}
                      testId={`plugin-toggle-${p.id}`}
                    />
                    {p.canRemove && (
                      <button
                        type="button"
                        onClick={() => setRemoving(p)}
                        disabled={busyId !== null}
                        aria-label={`Remove ${p.name}`}
                        data-testid={`plugin-remove-${p.id}`}
                        className="flex h-11 w-10 shrink-0 items-center justify-center border-0 bg-transparent p-0 text-danger disabled:opacity-50"
                      >
                        <Trash2 size={16} aria-hidden />
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            </Card>
          ))}
        </section>
      ))}

      <ConfirmSheet
        open={removing !== null}
        title={`Remove ${removing?.name ?? ''}?`}
        message="The add-on's files are deleted from this browser profile. A backup is saved first. The browser must be closed."
        confirmLabel="Remove"
        danger
        onConfirm={() => removing && void remove(removing)}
        onCancel={() => setRemoving(null)}
      />
    </div>
  );
}
