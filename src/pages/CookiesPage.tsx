import { useCallback, useEffect, useMemo, useState } from 'react';
import { ArrowRight, Search, ShieldCheck, Trash2, Undo2, Wand2 } from 'lucide-react';
import type { CookieDomain, DeleteResult, KeepList, ScanResult } from '../api/cookies';
import { Card } from '../components/Card';
import { Checkbox } from '../components/Checkbox';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { ErrorBanner } from '../components/ErrorBanner';
import { useCall } from '../hooks/useCall';
import { ApiCallError, call } from '../lib/transport';
import {
  addKeep,
  buildLists,
  removeKeep,
  selectedVisible,
  toggleSelected,
} from '../lib/cookieLists';

export default function CookiesPage() {
  const cookieCall = useCall<CookieDomain[]>('cookies.list');
  const keepCall = useCall<KeepList>('cookies.get_keep_list');
  // Newest keep list from a local edit; supersedes what the initial load returned.
  const [keepEdit, setKeepEdit] = useState<string[] | null>(null);
  const [actionError, setActionError] = useState<ApiCallError | null>(null);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const runCookies = cookieCall.run;
  const runKeep = keepCall.run;
  const load = useCallback(async () => {
    setKeepEdit(null);
    setActionError(null);
    await Promise.all([runCookies(), runKeep()]);
  }, [runCookies, runKeep]);

  useEffect(() => {
    void runCookies();
    void runKeep();
  }, [runCookies, runKeep]);

  const cookies = cookieCall.data;
  const keep = useMemo(() => keepEdit ?? keepCall.data?.domains ?? [], [keepEdit, keepCall.data]);
  const error = actionError ?? cookieCall.error ?? keepCall.error;
  const setError = setActionError;

  const lists = useMemo(() => buildLists(cookies ?? [], keep, query), [cookies, keep, query]);
  const chosen = useMemo(() => selectedVisible(selected, lists.all), [selected, lists.all]);

  const saveKeep = async (next: string[]) => {
    setKeepEdit(next); // optimistic: the move feels instant
    try {
      const res = await call<KeepList>('cookies.set_keep_list', { domains: next });
      setKeepEdit(res.domains);
      setError(null);
    } catch (e) {
      setKeepEdit(null);
      setError(ApiCallError.from(e));
    }
  };

  const smartKeep = async () => {
    setBusy(true);
    try {
      const res = await call<ScanResult>('cookies.intelligent_scan');
      setKeepEdit(res.domains);
      setNote(
        res.added.length === 0
          ? 'No new well-known sites found.'
          : `Keeping ${res.added.length} well-known ${res.added.length === 1 ? 'site' : 'sites'}: ${res.added.join(', ')}.`,
      );
      setError(null);
    } catch (e) {
      setError(ApiCallError.from(e));
    } finally {
      setBusy(false);
    }
  };

  const doDelete = async () => {
    setConfirmDelete(false);
    setBusy(true);
    try {
      const res = await call<DeleteResult>('cookies.delete', { domains: chosen });
      const skipped = res.browsers.filter((b) => b.skipped);
      setNote(
        `Deleted ${res.deleted} ${res.deleted === 1 ? 'cookie' : 'cookies'}.` +
          (skipped.length > 0
            ? ` Skipped ${skipped.map((b) => b.browser).join(', ')} (${skipped[0]?.skipped === 'in_use' ? 'in use' : 'running'}).`
            : ''),
      );
      setSelected(new Set());
      await load();
    } catch (e) {
      setError(ApiCallError.from(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div data-testid="page-cookies" className="flex flex-col gap-3 p-4">
      <ErrorBanner error={error} onRetry={() => void load()} />

      <div className="flex gap-2">
        <label className="relative min-w-0 flex-1">
          <Search size={15} className="pointer-events-none absolute left-2.5 top-3 text-muted" aria-hidden />
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search sites"
            aria-label="Search sites"
            data-testid="cookies-search"
            className="h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 pl-8 pr-2 text-sm"
          />
        </label>
        <button
          type="button"
          onClick={() => void smartKeep()}
          disabled={busy || cookies === null}
          data-testid="btn-smart-keep"
          className="flex h-10 shrink-0 items-center gap-1.5 rounded-xl border border-line bg-surface-2 px-3 text-sm font-medium disabled:opacity-50"
        >
          <Wand2 size={15} aria-hidden /> Smart keep
        </button>
      </div>

      {note && (
        <p className="m-0 break-words rounded-xl border border-line bg-surface p-2 text-xs" data-testid="cookies-note">
          {note}
        </p>
      )}

      {cookies === null && !error && <p className="py-6 text-center text-sm text-muted">Reading cookies...</p>}

      {cookies !== null && (
        <>
          <Card title={`Cookies on this computer (${lists.all.length})`} testId="cookies-all" className="!p-0">
            {lists.all.length === 0 ? (
              <p className="m-0 p-3 text-sm text-muted" data-testid="cookies-all-empty">
                {cookies.length === 0 ? 'No cookies found in your browsers.' : 'Nothing here.'}
              </p>
            ) : (
              <ul className="m-0 list-none divide-y divide-line p-0">
                {lists.all.map((c) => (
                  <li key={c.domain} className="flex min-w-0 items-center gap-2 px-3 py-1" data-testid={`cookie-${c.domain}`}>
                    <Checkbox
                      checked={selected.has(c.domain)}
                      onChange={() => setSelected((s) => toggleSelected(s, c.domain))}
                      ariaLabel={`Select ${c.domain}`}
                      testId={`cookie-select-${c.domain}`}
                    />
                    <button
                      type="button"
                      onClick={() => void saveKeep(addKeep(keep, c.domain))}
                      aria-label={`Keep ${c.domain}`}
                      data-testid={`cookie-keep-${c.domain}`}
                      className="flex min-h-10 min-w-0 flex-1 items-center gap-2 border-0 bg-transparent p-0 text-left"
                    >
                      <span className="min-w-0 flex-1">
                        <span className="block break-all text-sm">{c.domain}</span>
                        <span className="block text-xs text-muted">
                          {c.count} {c.count === 1 ? 'cookie' : 'cookies'} - {c.browsers.join(', ')}
                        </span>
                      </span>
                      <ArrowRight size={16} className="shrink-0 text-muted" aria-hidden />
                    </button>
                  </li>
                ))}
              </ul>
            )}
            {chosen.length > 0 && (
              <div className="border-t border-line p-2">
                <button
                  type="button"
                  onClick={() => setConfirmDelete(true)}
                  disabled={busy}
                  data-testid="btn-cookies-delete"
                  className="flex h-10 w-full items-center justify-center gap-1.5 rounded-xl border-0 bg-danger text-sm font-semibold text-danger-fg disabled:opacity-50"
                >
                  <Trash2 size={16} aria-hidden /> Delete selected ({chosen.length})
                </button>
              </div>
            )}
          </Card>

          <Card title={`Cookies to keep (${lists.keep.length})`} testId="cookies-keep" className="!p-0">
            {lists.keep.length === 0 ? (
              <p className="m-0 p-3 text-sm text-muted" data-testid="cookies-keep-empty">
                Tap a site above to keep its cookies when cleaning, or use Smart keep.
              </p>
            ) : (
              <ul className="m-0 list-none divide-y divide-line p-0">
                {lists.keep.map((k) => (
                  <li key={k.domain} data-testid={`keep-${k.domain}`}>
                    <button
                      type="button"
                      onClick={() => void saveKeep(removeKeep(keep, k.domain))}
                      aria-label={`Stop keeping ${k.domain}`}
                      data-testid={`keep-remove-${k.domain}`}
                      className="flex min-h-10 w-full min-w-0 items-center gap-2 border-0 bg-transparent px-3 py-1 text-left"
                    >
                      <ShieldCheck size={16} className="shrink-0 text-ok" aria-hidden />
                      <span className="min-w-0 flex-1">
                        <span className="block break-all text-sm">{k.domain}</span>
                        <span className="block text-xs text-muted">
                          {k.count} {k.count === 1 ? 'cookie' : 'cookies'} protected (includes subdomains)
                        </span>
                      </span>
                      <Undo2 size={16} className="shrink-0 text-muted" aria-hidden />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </Card>
        </>
      )}

      <ConfirmSheet
        open={confirmDelete}
        title={`Delete cookies of ${chosen.length} ${chosen.length === 1 ? 'site' : 'sites'}?`}
        message="This removes them (and their subdomains) from every browser and signs you out of those sites."
        confirmLabel="Delete"
        danger
        onConfirm={() => void doDelete()}
        onCancel={() => setConfirmDelete(false)}
      />
    </div>
  );
}
