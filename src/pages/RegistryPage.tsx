import { Archive, RotateCcw, ScanSearch } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import type {
  CategoriesResult,
  FixParams,
  FixResult,
  Issue,
  RestoreOutcome,
  ScanParams,
  ScanResult,
} from '../api/registry_cleaner';
import { Card } from '../components/Card';
import { Checkbox } from '../components/Checkbox';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { EmptyState } from '../components/EmptyState';
import { ErrorBanner } from '../components/ErrorBanner';
import { RunProgress } from '../components/RunProgress';
import { useCall } from '../hooks/useCall';
import {
  confirmMessage,
  defaultCategories,
  failures,
  fixSummary,
  groupIssues,
  plural,
  preselectIssues,
  pruneSelection,
  setMany,
} from '../lib/registry';
import { IssueGroups } from './registry/IssueGroups';
import { BackupsView } from './registry/BackupsView';

export default function RegistryPage() {
  const cats = useCall<CategoriesResult>('registry_cleaner.categories');
  const scan = useCall<ScanResult, ScanParams>('registry_cleaner.scan');
  const fix = useCall<FixResult, FixParams>('registry_cleaner.fix');
  const restore = useCall<RestoreOutcome, { id: string }>('registry_cleaner.restore_backup');

  const [view, setView] = useState<'scan' | 'backups'>('scan');
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [issues, setIssues] = useState<Issue[] | null>(null);
  const [scanned, setScanned] = useState<string[]>([]);
  const [skipped, setSkipped] = useState<ScanResult['skipped']>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [confirmFix, setConfirmFix] = useState(false);
  const [confirmRestore, setConfirmRestore] = useState(false);
  const [result, setResult] = useState<{ fix: FixResult; failed: ReturnType<typeof failures> } | null>(null);
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);

  const loadCats = cats.run;
  useEffect(() => {
    void loadCats().then((c) => {
      if (c) setChosen(defaultCategories(c.categories));
    });
  }, [loadCats]);

  const categories = useMemo(() => cats.data?.categories ?? [], [cats.data]);
  const platform = cats.data?.platform ?? 'linux';
  const windows = platform === 'windows';
  const noun = windows ? 'registry' : 'configuration';

  const groups = useMemo(() => (issues ? groupIssues(issues, categories) : []), [issues, categories]);
  const running = scan.loading || fix.loading || restore.loading;
  const active = scan.loading ? scan : fix.loading ? fix : restore.loading ? restore : null;
  const activeLabel = scan.loading ? 'Scanning' : fix.loading ? 'Fixing' : 'Restoring';
  const selectedIssues = useMemo(() => (issues ?? []).filter((i) => selected.has(i.id)), [issues, selected]);
  const adminCount = selectedIssues.filter((i) => i.needsAdmin).length;

  const toggleCategory = (id: string) =>
    setChosen((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  const doScan = async () => {
    setResult(null);
    setNote(null);
    const ids = categories.filter((c) => chosen.has(c.id)).map((c) => c.id);
    const r = await scan.run({ categories: ids });
    if (!r) return;
    setIssues(r.issues);
    setScanned(r.scanned);
    setSkipped(r.skipped);
    setSelected(preselectIssues(r.issues, categories));
  };

  const doFix = async () => {
    setConfirmFix(false);
    setNote(null);
    const r = await fix.run({ issueIds: [...selected], backup: true, categories: scanned });
    if (!r || !issues) return;
    const okIds = new Set(r.results.filter((x) => x.ok).map((x) => x.id));
    const remaining = issues.filter((i) => !okIds.has(i.id));
    setResult({ fix: r, failed: failures(r, issues) });
    setIssues(remaining);
    setSelected((s) => pruneSelection(s, remaining));
  };

  const doRestore = async () => {
    setConfirmRestore(false);
    const id = result?.fix.backupId;
    if (!id) return;
    const r = await restore.run({ id });
    if (r) {
      setNote({
        ok: r.ok,
        text: r.ok
          ? `Restored ${plural(r.restored, 'item')} from the backup. Scan again to see the current state.`
          : `${r.restored} restored, ${r.failed} could not be restored: ${r.results.find((x) => !x.ok)?.error ?? ''}`,
      });
      setResult(null);
    }
  };

  if (view === 'backups') {
    return (
      <div data-testid="page-registry" className="flex min-h-full flex-col gap-3 p-4">
        <BackupsView onBack={() => setView('scan')} />
      </div>
    );
  }

  const allIssues = issues ?? [];
  const allSelected = allIssues.length > 0 && selected.size === allIssues.length;
  const error = cats.error ?? scan.error ?? fix.error ?? restore.error;

  return (
    <div data-testid="page-registry" className="flex min-h-full flex-col gap-3 p-4">
      <ErrorBanner error={error} onRetry={cats.error ? () => void loadCats() : undefined} />

      <Card title={`What to check (${chosen.size} of ${categories.length})`} testId="registry-categories">
        <div className="mb-2 flex gap-3 text-xs">
          <button
            type="button"
            className="min-h-10 min-w-10 border-0 bg-transparent px-2 text-accent underline"
            onClick={() => setChosen(new Set(categories.map((c) => c.id)))}
            data-testid="registry-cats-all"
          >
            All
          </button>
          <button
            type="button"
            className="min-h-10 min-w-10 border-0 bg-transparent px-2 text-accent underline"
            onClick={() => setChosen(new Set())}
            data-testid="registry-cats-none"
          >
            None
          </button>
        </div>
        <ul className="m-0 grid list-none grid-cols-1 gap-1 p-0">
          {categories.map((c) => (
            <li key={c.id}>
              <label className="flex min-h-10 cursor-pointer items-center gap-2">
                <Checkbox
                  checked={chosen.has(c.id)}
                  onChange={() => toggleCategory(c.id)}
                  ariaLabel={c.label}
                  testId={`registry-category-${c.id}`}
                  disabled={running}
                />
                <span className="min-w-0 flex-1 break-words text-sm">{c.label}</span>
              </label>
            </li>
          ))}
        </ul>
      </Card>

      <button
        type="button"
        onClick={() => void doScan()}
        disabled={running || chosen.size === 0}
        data-testid="registry-scan"
        className="flex h-11 items-center justify-center gap-2 rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg disabled:opacity-50"
      >
        <ScanSearch size={16} aria-hidden /> Scan for issues
      </button>

      {active && (
        <RunProgress
          label={activeLabel}
          progress={active.progress}
          onCancel={active.cancel}
          testId="registry-progress"
        />
      )}

      {note && (
        <p
          role="status"
          data-testid="registry-note"
          className={`m-0 break-words rounded-xl border p-2 text-xs ${note.ok ? 'border-ok/40 bg-ok/10' : 'border-danger/40 bg-danger/10'}`}
        >
          {note.text}
        </p>
      )}

      {result && (
        <Card testId="registry-fix-result" title="Result">
          <p className="m-0 break-words text-sm" data-testid="registry-fix-summary">
            {fixSummary(result.fix)}
          </p>
          {result.failed.length > 0 && (
            <ul className="m-0 mt-2 list-disc pl-4 text-xs text-danger" data-testid="registry-fix-failures">
              {result.failed.slice(0, 20).map((f) => (
                <li key={f.id} className="break-words">
                  {f.description}: {f.error}
                </li>
              ))}
            </ul>
          )}
          {result.fix.backupId && (
            <div className="mt-3 flex flex-col gap-2">
              <p className="m-0 text-xs text-muted">A backup of the changed {noun} items was saved.</p>
              <button
                type="button"
                onClick={() => setConfirmRestore(true)}
                disabled={running}
                data-testid="registry-restore-backup"
                className="flex h-10 items-center justify-center gap-1 rounded-xl border border-line bg-surface-2 text-sm disabled:opacity-50"
              >
                <RotateCcw size={14} aria-hidden /> Restore backup
              </button>
            </div>
          )}
        </Card>
      )}

      {skipped.length > 0 && (
        <p className="m-0 break-words text-xs text-muted" data-testid="registry-skipped">
          Could not check: {skipped.map((s) => `${s.category} (${s.reason})`).join('; ')}
        </p>
      )}

      {issues !== null && allIssues.length === 0 && !scan.loading && (
        <div data-testid="registry-empty">
          <EmptyState
            title="No issues found"
            hint={windows ? 'The registry entries checked look fine.' : 'Nothing that needs fixing was found.'}
          />
        </div>
      )}

      {allIssues.length > 0 && (
        <>
          <div className="flex items-center justify-between gap-2" data-testid="registry-results">
            <label className="flex min-h-10 items-center gap-2 text-sm">
              <Checkbox
                checked={allSelected}
                indeterminate={selected.size > 0 && !allSelected}
                onChange={(on) => setSelected(setMany(selected, allIssues, on))}
                ariaLabel="Select all issues"
                testId="registry-select-all"
                disabled={running}
              />
              <span>Select all ({allIssues.length})</span>
            </label>
          </div>
          <IssueGroups groups={groups} selected={selected} onChange={setSelected} disabled={running} />
          <button
            type="button"
            onClick={() => setConfirmFix(true)}
            disabled={running || selected.size === 0}
            data-testid="registry-fix"
            className="sticky bottom-2 h-11 rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg shadow disabled:opacity-50"
          >
            Fix selected issues ({selected.size})
          </button>
        </>
      )}

      <button
        type="button"
        onClick={() => setView('backups')}
        data-testid="registry-open-backups"
        className="mt-auto flex h-10 items-center justify-center gap-1 rounded-xl border border-line bg-surface-2 text-sm"
      >
        <Archive size={14} aria-hidden /> Backups
      </button>

      <ConfirmSheet
        open={confirmFix}
        title={`Fix ${plural(selected.size, windows ? 'registry entry' : 'issue', windows ? 'registry entries' : 'issues')}?`}
        message={confirmMessage(selected.size, adminCount, windows)}
        confirmLabel="Back up and fix"
        onConfirm={() => void doFix()}
        onCancel={() => setConfirmFix(false)}
      />
      <ConfirmSheet
        open={confirmRestore}
        title="Restore the backup?"
        message="The items changed by the last fix are put back as they were. You may be asked for administrator rights."
        confirmLabel="Restore"
        onConfirm={() => void doRestore()}
        onCancel={() => setConfirmRestore(false)}
      />
    </div>
  );
}
