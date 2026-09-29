import { Copy, Download, Loader2, Search, Settings2, Trash2, Wand2 } from 'lucide-react';
import { useMemo, useState } from 'react';
import type {
  AutoRule,
  AutoSelectResult,
  DupDeleteResult,
  DupGroup,
  DupScanParams,
  DupScanResult,
  ExportResult,
} from '../api/duplicates';
import { BottomSheet } from '../components/BottomSheet';
import { Card } from '../components/Card';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { EmptyState } from '../components/EmptyState';
import { ErrorBanner } from '../components/ErrorBanner';
import { RunProgress } from '../components/RunProgress';
import { useCall } from '../hooks/useCall';
import { saveText } from '../lib/download';
import { DEFAULT_OPTIONS, matchSummary, toScanParams, type DupOptions } from '../lib/dupOptions';
import {
  blockedGroups,
  canDelete,
  fromAuto,
  groupsTouched,
  removeFiles,
  selectedBytes,
  selectedPaths,
} from '../lib/dupSelection';
import { formatBytes } from '../lib/format';
import { ApiCallError, call } from '../lib/transport';
import { GroupList } from './duplicates/GroupList';
import { OptionsSheet } from './duplicates/OptionsSheet';

const RULES: { rule: Exclude<AutoRule, 'keep_in_folder'>; label: string; hint: string }[] = [
  { rule: 'keep_newest', label: 'Keep the newest', hint: 'Select every older copy' },
  { rule: 'keep_oldest', label: 'Keep the oldest', hint: 'Select every newer copy' },
  { rule: 'keep_shortest_path', label: 'Keep the shortest path', hint: 'Usually the original location' },
];

export default function DuplicateFinderPage() {
  const scan = useCall<DupScanResult, DupScanParams>('duplicates.scan');
  const [options, setOptions] = useState<DupOptions>(DEFAULT_OPTIONS);
  const [optionsOpen, setOptionsOpen] = useState(false);
  const [autoOpen, setAutoOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [folderRule, setFolderRule] = useState('');
  const [groups, setGroups] = useState<DupGroup[] | null>(null);
  const [scanId, setScanId] = useState<string | null>(null);
  const [info, setInfo] = useState<DupScanResult | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [failures, setFailures] = useState<string[]>([]);
  const [formError, setFormError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<ApiCallError | null>(null);

  const list = useMemo(() => groups ?? [], [groups]);
  const chosen = useMemo(() => selectedPaths(list, selected), [list, selected]);
  const chosenBytes = useMemo(() => selectedBytes(list, selected), [list, selected]);
  const touched = useMemo(() => groupsTouched(list, selected), [list, selected]);
  const deletable = canDelete(list, selected);
  const blocked = blockedGroups(list, selected);
  const wasted = list.reduce((s, g) => s + g.wastedBytes, 0);
  const result = info;

  const start = async () => {
    const built = toScanParams(options);
    if ('error' in built) {
      setFormError(built.error);
      setOptionsOpen(true);
      return;
    }
    setFormError(null);
    setNote(null);
    setFailures([]);
    setActionError(null);
    setSelected(new Set());
    setGroups(null);
    setInfo(null);
    const r = await scan.run(built.params);
    if (r) {
      setGroups(r.groups);
      setScanId(r.scanId);
      setInfo(r);
    }
  };

  const applyRule = async (rule: AutoRule, folder?: string) => {
    if (!scanId) return;
    setBusy(true);
    setActionError(null);
    try {
      const r = await call<AutoSelectResult>('duplicates.auto_select', { scanId, rule, folder });
      const next = fromAuto(list, r);
      setSelected(next);
      setAutoOpen(false);
      setNote(
        next.size === 0
          ? 'The rule did not select anything.'
          : `Selected ${next.size} ${next.size === 1 ? 'file' : 'files'} (${formatBytes(selectedBytes(list, next))}). Every group keeps at least one copy.`,
      );
    } catch (e) {
      setActionError(ApiCallError.from(e));
    } finally {
      setBusy(false);
    }
  };

  const doDelete = async () => {
    if (!scanId) return;
    setConfirm(false);
    setBusy(true);
    setActionError(null);
    setNote(null);
    setFailures([]);
    try {
      const r = await call<DupDeleteResult>('duplicates.delete', { scanId, paths: chosen });
      const gone = new Set(r.results.filter((x) => x.ok).map((x) => x.path));
      setGroups((g) => removeFiles(g ?? [], gone));
      setSelected((s) => new Set([...s].filter((p) => !gone.has(p))));
      const failed = r.results.filter((x) => !x.ok);
      setFailures(failed.map((f) => `${f.path}: ${f.error ?? 'failed'}`));
      setNote(
        `Deleted ${r.deleted} ${r.deleted === 1 ? 'file' : 'files'}, freed ${formatBytes(r.freedBytes)}.` +
          (failed.length > 0 ? ` ${failed.length} could not be deleted.` : ''),
      );
    } catch (e) {
      setActionError(ApiCallError.from(e));
    } finally {
      setBusy(false);
    }
  };

  const doExport = async (format: 'csv' | 'txt') => {
    if (!scanId) return;
    setBusy(true);
    setActionError(null);
    try {
      const r = await call<ExportResult>('duplicates.export', { scanId, format });
      const how = await saveText(r.filename, r.text, format === 'csv' ? 'text/csv' : 'text/plain');
      setExportOpen(false);
      setNote(
        how === 'downloaded'
          ? `Saved ${r.filename}.`
          : how === 'copied'
            ? 'The report was copied to the clipboard.'
            : 'Could not save the report.',
      );
    } catch (e) {
      setActionError(ApiCallError.from(e));
    } finally {
      setBusy(false);
    }
  };

  const error = actionError ?? scan.error;

  return (
    <div data-testid="page-duplicates" className="flex min-h-full flex-col gap-3 p-4">
      <ErrorBanner error={error} />
      {formError && (
        <p role="alert" className="m-0 text-sm text-danger" data-testid="dup-form-error">
          {formError}
        </p>
      )}

      <Card title="Search">
        <p className="m-0 break-words text-sm" data-testid="dup-options-summary">
          {options.paths.length === 0
            ? 'No folders chosen yet.'
            : `${options.paths.length} ${options.paths.length === 1 ? 'folder' : 'folders'}, match by ${matchSummary(options.match)}`}
        </p>
        <div className="mt-2 flex gap-2">
          <button
            type="button"
            onClick={() => setOptionsOpen(true)}
            disabled={scan.loading}
            data-testid="btn-dup-options"
            className="flex h-10 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border border-line bg-surface-2 text-sm font-medium disabled:opacity-50"
          >
            <Settings2 size={16} aria-hidden /> Options
          </button>
          <button
            type="button"
            onClick={() => void start()}
            disabled={scan.loading || busy}
            data-testid="btn-dup-scan"
            className="flex h-10 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg disabled:opacity-50"
          >
            {scan.loading ? <Loader2 size={16} className="animate-spin" aria-hidden /> : <Search size={16} aria-hidden />}
            Find
          </button>
        </div>
      </Card>

      {scan.loading && (
        <RunProgress label="Searching" progress={scan.progress} onCancel={scan.cancel} testId="dup-progress" />
      )}

      {result && groups && !scan.loading && (
        <>
          <p className="m-0 text-xs text-muted" data-testid="dup-summary">
            {list.length} {list.length === 1 ? 'group' : 'groups'} of duplicates, {formatBytes(wasted)} can be freed
            {' '}({result.scannedFiles} files checked in {(result.durationMs / 1000).toFixed(1)}s)
            {result.errors.count > 0 && ` - ${result.errors.count} items could not be read`}
          </p>
          {result.truncatedGroups && (
            <p className="m-0 text-xs text-warn" data-testid="dup-truncated">
              Showing the {result.groups.length} groups that waste the most space.
            </p>
          )}

          {list.length > 0 && (
            <div className="flex gap-2">
              <button
                type="button"
                onClick={() => setAutoOpen(true)}
                disabled={busy}
                data-testid="btn-dup-auto"
                className="flex h-9 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border border-line bg-surface-2 text-sm font-medium disabled:opacity-50"
              >
                <Wand2 size={15} aria-hidden /> Auto select
              </button>
              <button
                type="button"
                onClick={() => setExportOpen(true)}
                disabled={busy}
                data-testid="btn-dup-export"
                className="flex h-9 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border border-line bg-surface-2 text-sm font-medium disabled:opacity-50"
              >
                <Download size={15} aria-hidden /> Export
              </button>
            </div>
          )}

          {note && (
            <p className="m-0 break-words rounded-xl border border-line bg-surface p-2 text-xs" data-testid="dup-note">
              {note}
            </p>
          )}
          {failures.length > 0 && (
            <ul className="m-0 list-none space-y-1 p-0 text-xs text-danger" data-testid="dup-failures">
              {failures.slice(0, 5).map((f) => (
                <li key={f} className="break-all">
                  {f}
                </li>
              ))}
              {failures.length > 5 && <li>...and {failures.length - 5} more</li>}
            </ul>
          )}

          {list.length === 0 ? (
            <EmptyState icon={Copy} title="No duplicates left" hint="Nothing to clean up here." />
          ) : (
            <GroupList groups={list} selected={selected} onChange={setSelected} />
          )}

          <div
            data-testid="dup-action-bar"
            className="sticky bottom-0 z-10 mt-auto flex flex-col gap-1 rounded-xl border border-line bg-surface p-2 shadow-lg"
          >
            {blocked.length > 0 && (
              <p className="m-0 text-xs text-danger" data-testid="dup-blocked">
                Every copy in {blocked.length} {blocked.length === 1 ? 'group is' : 'groups are'} selected. Keep at least one.
              </p>
            )}
            <div className="flex gap-2">
              <button
                type="button"
                onClick={() => setSelected(new Set())}
                disabled={selected.size === 0 || busy}
                data-testid="btn-dup-clear"
                className="h-10 shrink-0 rounded-xl border border-line bg-surface-2 px-3 text-sm font-medium disabled:opacity-50"
              >
                Clear
              </button>
              <button
                type="button"
                onClick={() => setConfirm(true)}
                disabled={!deletable || busy}
                data-testid="btn-dup-delete"
                className="flex h-10 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border-0 bg-danger text-sm font-semibold text-white disabled:opacity-50"
              >
                {busy ? <Loader2 size={16} className="animate-spin" aria-hidden /> : <Trash2 size={16} aria-hidden />}
                <span className="truncate">
                  Delete selected ({chosen.length}){chosen.length > 0 ? ` ${formatBytes(chosenBytes)}` : ''}
                </span>
              </button>
            </div>
          </div>
        </>
      )}

      {!result && !scan.loading && !scan.error && (
        <EmptyState
          icon={Copy}
          title="Find duplicate files"
          hint="Choose folders in Options, then tap Find. Nothing is deleted until you confirm."
        />
      )}

      <OptionsSheet open={optionsOpen} options={options} onChange={setOptions} onClose={() => setOptionsOpen(false)} />

      <BottomSheet open={autoOpen} title="Auto select" onClose={() => setAutoOpen(false)} testId="dup-auto">
        <div className="flex flex-col gap-2">
          {RULES.map((r) => (
            <button
              key={r.rule}
              type="button"
              onClick={() => void applyRule(r.rule)}
              disabled={busy}
              data-testid={`dup-rule-${r.rule}`}
              className="min-h-11 rounded-xl border border-line bg-surface-2 px-3 py-2 text-left text-sm font-medium disabled:opacity-50"
            >
              {r.label}
              <span className="block text-[11px] font-normal text-muted">{r.hint}</span>
            </button>
          ))}
          <div className="rounded-xl border border-line bg-surface-2 p-3">
            <p className="m-0 text-sm font-medium">Keep files in a folder</p>
            <p className="m-0 mb-2 text-[11px] text-muted">Select copies outside it. Groups with no copy in the folder are left alone.</p>
            <div className="flex gap-2">
              <input
                type="text"
                value={folderRule}
                onChange={(e) => setFolderRule(e.target.value)}
                placeholder="Folder to keep"
                aria-label="Folder to keep"
                data-testid="dup-rule-folder-input"
                className="h-10 min-w-0 flex-1 rounded-xl border border-line bg-surface px-3 text-sm"
              />
              <button
                type="button"
                disabled={!folderRule.trim() || busy}
                onClick={() => void applyRule('keep_in_folder', folderRule.trim())}
                data-testid="dup-rule-keep_in_folder"
                className="h-10 shrink-0 rounded-xl border-0 bg-accent px-3 text-sm font-semibold text-accent-fg disabled:opacity-50"
              >
                Apply
              </button>
            </div>
          </div>
        </div>
      </BottomSheet>

      <BottomSheet open={exportOpen} title="Export report" onClose={() => setExportOpen(false)} testId="dup-export">
        <div className="flex flex-col gap-2">
          {(['csv', 'txt'] as const).map((f) => (
            <button
              key={f}
              type="button"
              onClick={() => void doExport(f)}
              disabled={busy}
              data-testid={`dup-export-${f}`}
              className="min-h-11 rounded-xl border border-line bg-surface-2 px-3 py-2 text-left text-sm font-medium disabled:opacity-50"
            >
              {f === 'csv' ? 'Spreadsheet (.csv)' : 'Plain text (.txt)'}
            </button>
          ))}
        </div>
      </BottomSheet>

      <ConfirmSheet
        open={confirm}
        title={`Delete ${chosen.length} ${chosen.length === 1 ? 'file' : 'files'}?`}
        message={`${formatBytes(chosenBytes)} in ${touched} ${touched === 1 ? 'group' : 'groups'} will be permanently deleted. At least one copy of each stays. This cannot be undone.`}
        confirmLabel="Delete"
        danger
        onConfirm={() => void doDelete()}
        onCancel={() => setConfirm(false)}
      />
    </div>
  );
}
