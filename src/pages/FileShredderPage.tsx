import { CircleAlert, CircleCheck, FileX, Loader2, Plus, ShieldAlert, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import type { SecureDeleteParams, SecureDeleteResult } from '../api/secure_delete';
import type { Passes, Settings } from '../api/settings';
import { Card } from '../components/Card';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { ErrorBanner } from '../components/ErrorBanner';
import { RunProgress } from '../components/RunProgress';
import { useCall } from '../hooks/useCall';
import { formatBytes } from '../lib/format';
import { PASS_OPTIONS, isPasses } from '../lib/wipe';
import { SHRED_WORD, parsePaths, pathProblem, summarize } from '../lib/shred';

/** Overwrite files and folders so they cannot be recovered, then remove them. */
export default function FileShredderPage() {
  const shred = useCall<SecureDeleteResult, SecureDeleteParams>('secure_delete.delete');
  const settingsCall = useCall<Settings>('settings.get');
  const [paths, setPaths] = useState<string[]>([]);
  const [input, setInput] = useState('');
  const [inputError, setInputError] = useState<string | null>(null);
  const [passes, setPasses] = useState<Passes>(1);
  const [passesTouched, setPassesTouched] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [report, setReport] = useState<SecureDeleteResult | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const resultRef = useRef<HTMLDivElement>(null);

  const loadSettings = settingsCall.run;
  useEffect(() => {
    void loadSettings();
  }, [loadSettings]);
  const defaultPasses = settingsCall.data?.secureDelete.passes;
  // The saved default applies until the user picks a value themselves.
  const shownPasses: Passes = passesTouched || !defaultPasses ? passes : defaultPasses;

  const running = shred.loading;

  const add = () => {
    const parsed = parsePaths(input);
    if (parsed.length === 0) {
      setInputError('Enter a path');
      return;
    }
    const bad = parsed.map((p) => ({ p, why: pathProblem(p) })).find((x) => x.why);
    if (bad) {
      setInputError(`${bad.why}: ${bad.p}`);
      return;
    }
    setPaths((cur) => [...cur, ...parsed.filter((p) => !cur.includes(p))]);
    setInput('');
    setInputError(null);
    setReport(null);
    inputRef.current?.focus();
  };

  const remove = (p: string) => setPaths((cur) => cur.filter((x) => x !== p));

  const start = async () => {
    setConfirm(false);
    setReport(null);
    const r = await shred.run({ paths, passes: shownPasses });
    if (r) {
      setReport(r);
      // Keep what failed so the user can see it (and retry); drop what is gone.
      const failed = new Set(r.results.filter((x) => !x.ok).map((x) => x.path));
      setPaths((cur) => cur.filter((p) => failed.has(p)));
    }
  };

  // The result is below the form: bring it into view when it arrives.
  useEffect(() => {
    if (report) resultRef.current?.scrollIntoView?.({ block: 'start' });
  }, [report]);

  const summary = report ? summarize(report.results) : null;
  const passWord = shownPasses === 1 ? 'pass' : 'passes';

  return (
    <div data-testid="page-shredder" className="flex min-h-full flex-col gap-3 p-4">
      <ErrorBanner error={shred.error ?? settingsCall.error} />

      <div
        role="note"
        data-testid="shredder-warning"
        className="flex min-w-0 items-start gap-2 rounded-xl border border-danger/40 bg-danger/10 p-3 text-sm"
      >
        <ShieldAlert size={18} className="mt-0.5 shrink-0 text-danger" aria-hidden />
        <div className="min-w-0">
          <p className="m-0 font-semibold">Shredded files cannot be recovered.</p>
          <p className="m-0 mt-1 text-xs">
            Each file is overwritten, renamed and removed; folders are shredded file by file. There is no undo and
            nothing goes to the trash. On SSDs, snapshots and copy-on-write filesystems overwriting is best effort.
          </p>
        </div>
      </div>

      <Card title="Files and folders" testId="shredder-input-card">
        <label htmlFor="shredder-path" className="mb-1 block text-xs text-muted">
          Absolute path (one per line; paste several at once)
        </label>
        <div className="flex items-start gap-2">
          <textarea
            id="shredder-path"
            ref={inputRef}
            rows={2}
            value={input}
            disabled={running}
            onChange={(e) => {
              setInput(e.target.value);
              setInputError(null);
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                add();
              }
            }}
            placeholder="/home/me/old-tax-return.pdf"
            autoComplete="off"
            autoCapitalize="off"
            spellCheck={false}
            data-testid="shredder-input"
            aria-invalid={inputError ? true : undefined}
            aria-describedby={inputError ? 'shredder-input-error' : undefined}
            className="min-h-11 min-w-0 flex-1 resize-none break-all rounded-xl border border-line bg-surface-2 px-3 py-2 font-mono text-xs"
          />
          <button
            type="button"
            onClick={add}
            disabled={running || !input.trim()}
            aria-label="Add path"
            data-testid="shredder-add"
            className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-line bg-surface-2 disabled:opacity-50"
          >
            <Plus size={18} aria-hidden />
          </button>
        </div>
        {inputError && (
          <p id="shredder-input-error" role="alert" className="m-0 mt-1 break-words text-xs text-danger" data-testid="shredder-input-error">
            {inputError}
          </p>
        )}
        <p className="m-0 mt-2 text-[11px] text-muted">
          A file picker is not available in the browser, so paste or type the path. Operating system folders and other
          protected locations are refused.
        </p>

        {paths.length > 0 && (
          <ul className="m-0 mt-3 flex list-none flex-col gap-1 p-0" data-testid="shredder-list" aria-label="Selected for shredding">
            {paths.map((p) => (
              <li
                key={p}
                data-testid="shredder-item"
                className="flex min-w-0 items-center gap-1 rounded-lg bg-surface-2 pl-3"
              >
                <span className="min-w-0 flex-1 break-all py-2 font-mono text-xs">{p}</span>
                <button
                  type="button"
                  onClick={() => remove(p)}
                  disabled={running}
                  aria-label={`Remove ${p}`}
                  className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full border-0 bg-transparent disabled:opacity-50"
                >
                  <X size={16} aria-hidden />
                </button>
              </li>
            ))}
          </ul>
        )}
      </Card>

      <Card title="Overwrite passes">
        <label className="sr-only" htmlFor="shredder-passes">
          Overwrite passes
        </label>
        <select
          id="shredder-passes"
          value={shownPasses}
          disabled={running}
          onChange={(e) => {
            const n = Number(e.target.value);
            if (isPasses(n)) {
              setPasses(n);
              setPassesTouched(true);
            }
          }}
          data-testid="shredder-passes"
          className="h-11 w-full rounded-xl border border-line bg-surface-2 px-3 text-sm"
        >
          {PASS_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label} - {o.hint}
            </option>
          ))}
        </select>
        <p className="m-0 mt-2 text-[11px] text-muted">
          More passes take proportionally longer. The default comes from Settings &rsaquo; Cleaning.
        </p>
      </Card>

      {running && (
        <RunProgress
          label="Shredding"
          progress={shred.progress}
          onCancel={shred.cancel}
          testId="shredder-progress"
          note="Cancelling keeps the remaining files, but a file that is partly overwritten stays damaged."
        />
      )}

      {report && summary && !running && (
        <div ref={resultRef} className="scroll-mt-2">
        <Card title="Result" testId="shredder-result">
          <p role="status" className="m-0 mb-2 break-words text-sm" data-testid="shredder-summary">
            {summary.done > 0 && `Shredded ${summary.done} ${summary.done === 1 ? 'item' : 'items'} (${formatBytes(summary.bytes)}).`}
            {summary.done > 0 && summary.failed > 0 && ' '}
            {summary.failed > 0 && `${summary.failed} could not be shredded.`}
          </p>
          <ul className="m-0 flex list-none flex-col gap-1 p-0">
            {report.results.map((r) => (
              <li
                key={r.path}
                data-testid={r.ok ? 'shredder-result-ok' : 'shredder-result-failed'}
                className="flex min-w-0 items-start gap-2 text-xs"
              >
                {r.ok ? (
                  <CircleCheck size={16} className="mt-0.5 shrink-0 text-ok" aria-label="Shredded" />
                ) : (
                  <CircleAlert size={16} className="mt-0.5 shrink-0 text-danger" aria-label="Failed" />
                )}
                <span className="min-w-0 flex-1">
                  <span className="block break-all font-mono">{r.path}</span>
                  <span className={`block break-words ${r.ok ? 'text-muted' : 'text-danger'}`}>
                    {r.ok ? formatBytes(r.bytes) : (r.error ?? 'Failed')}
                  </span>
                </span>
              </li>
            ))}
          </ul>
        </Card>
        </div>
      )}

      {paths.length === 0 && !report && !running && (
        <p className="m-0 flex items-center justify-center gap-2 py-4 text-center text-sm text-muted" data-testid="shredder-empty">
          <FileX size={18} aria-hidden /> Nothing added yet.
        </p>
      )}

      <div className="sticky bottom-0 z-10 mt-auto rounded-xl border border-line bg-surface p-2 shadow-lg">
        <button
          type="button"
          onClick={() => setConfirm(true)}
          disabled={paths.length === 0 || running}
          data-testid="btn-shredder-start"
          className="flex h-11 w-full items-center justify-center gap-1.5 rounded-xl border-0 bg-danger text-sm font-semibold text-danger-fg disabled:opacity-50"
        >
          {running ? <Loader2 size={16} className="animate-spin" aria-hidden /> : <FileX size={16} aria-hidden />}
          {paths.length > 0 ? `Shred ${paths.length} ${paths.length === 1 ? 'item' : 'items'}` : 'Shred'}
        </button>
      </div>

      <ConfirmSheet
        open={confirm}
        title={`Shred ${paths.length} ${paths.length === 1 ? 'item' : 'items'} for good?`}
        message={`${paths.length === 1 ? (paths[0] ?? '') : `${paths.length} files and folders`} will be overwritten ${shownPasses} ${passWord} and removed. Folders are shredded with everything inside. This cannot be undone.`}
        confirmLabel="Shred"
        danger
        requireText={SHRED_WORD}
        onConfirm={() => void start()}
        onCancel={() => setConfirm(false)}
      />
    </div>
  );
}
