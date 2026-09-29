import { ChevronLeft, FolderOpen, Loader2, Trash2 } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import type {
  DeleteFilesResult,
  FileCategory,
  FileRow,
  FileSort,
  FilesPage,
  FilesParams,
} from '../../api/disk_analyzer';
import { Checkbox } from '../../components/Checkbox';
import { ConfirmSheet } from '../../components/ConfirmSheet';
import { ErrorBanner } from '../../components/ErrorBanner';
import { CATEGORY_LABEL, formatDate, splitPath } from '../../lib/diskView';
import { formatBytes } from '../../lib/format';
import { useCall } from '../../hooks/useCall';
import { ApiCallError, call } from '../../lib/transport';

const PAGE = 100;

export interface FileFilter {
  category?: FileCategory;
  folder?: string;
}

interface Props {
  scanId: string;
  filter: FileFilter;
  title: string;
  onBack: () => void;
  onDeleted: (r: DeleteFilesResult) => void;
}

/** Paged, sortable file list with selection, delete and "open folder". */
export function FileList({ scanId, filter, title, onBack, onDeleted }: Props) {
  const [sort, setSort] = useState<FileSort>('size');
  const first = useCall<FilesPage, FilesParams>('disk_analyzer.files');
  // Pages after the first, and files deleted since the first page was fetched.
  const [more, setMore] = useState<FileRow[]>([]);
  const [removed, setRemoved] = useState<Set<string>>(new Set());
  const [loadingMore, setLoadingMore] = useState(false);
  const [actionError, setActionError] = useState<ApiCallError | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [failures, setFailures] = useState<string[]>([]);

  const runFirst = first.run;
  useEffect(() => {
    void runFirst({ scanId, category: filter.category, folder: filter.folder, sort, offset: 0, limit: PAGE });
  }, [runFirst, scanId, filter.category, filter.folder, sort]);

  const rows = useMemo(
    () => [...(first.data?.files ?? []), ...more].filter((r) => !removed.has(r.path)),
    [first.data, more, removed],
  );
  const total = Math.max(0, (first.data?.total ?? 0) - removed.size);
  const truncated = first.data?.truncated ?? false;
  const loading = first.loading || loadingMore;
  const error = actionError ?? first.error;

  const changeSort = (s: FileSort) => {
    setSort(s);
    setMore([]);
    setRemoved(new Set());
    setSelected(new Set());
  };

  const loadMore = async () => {
    setLoadingMore(true);
    try {
      const page = await call<FilesPage>('disk_analyzer.files', {
        scanId,
        category: filter.category,
        folder: filter.folder,
        sort,
        offset: rows.length,
        limit: PAGE,
      });
      setMore((m) => [...m, ...page.files]);
      setActionError(null);
    } catch (e) {
      setActionError(ApiCallError.from(e));
    } finally {
      setLoadingMore(false);
    }
  };

  const toggle = (path: string) =>
    setSelected((s) => {
      const n = new Set(s);
      if (n.has(path)) n.delete(path);
      else n.add(path);
      return n;
    });

  const allLoadedSelected = rows.length > 0 && rows.every((r) => selected.has(r.path));
  const selectedBytes = rows.filter((r) => selected.has(r.path)).reduce((s, r) => s + r.bytes, 0);

  const doDelete = async () => {
    setConfirm(false);
    setBusy(true);
    setNote(null);
    setFailures([]);
    try {
      const paths = [...selected];
      const res = await call<DeleteFilesResult>('disk_analyzer.delete', { scanId, paths });
      const gone = new Set(res.results.filter((r) => r.ok).map((r) => r.path));
      setRemoved((prev) => new Set([...prev, ...gone]));
      setSelected((s) => new Set([...s].filter((p) => !gone.has(p))));
      const failed = res.results.filter((r) => !r.ok);
      setFailures(failed.map((f) => `${f.path}: ${f.error ?? 'failed'}`));
      setNote(
        `Deleted ${res.deleted} ${res.deleted === 1 ? 'file' : 'files'}, freed ${formatBytes(res.freedBytes)}.` +
          (failed.length > 0 ? ` ${failed.length} could not be deleted.` : ''),
      );
      onDeleted(res);
      setActionError(null);
    } catch (e) {
      setActionError(ApiCallError.from(e));
    } finally {
      setBusy(false);
    }
  };

  const openFolder = async (path: string) => {
    try {
      await call('disk_analyzer.open_folder', { path });
      setActionError(null);
    } catch (e) {
      setActionError(ApiCallError.from(e));
    }
  };

  return (
    <div className="flex min-w-0 flex-col gap-2" data-testid="disk-files">
      <div className="flex min-w-0 items-center gap-2">
        <button
          type="button"
          onClick={onBack}
          aria-label="Back"
          data-testid="disk-files-back"
          className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full border border-line bg-surface-2"
        >
          <ChevronLeft size={18} aria-hidden />
        </button>
        <div className="min-w-0 flex-1">
          <p className="m-0 truncate text-sm font-semibold" data-testid="disk-files-title">
            {title}
          </p>
          <p className="m-0 text-xs text-muted" data-testid="disk-files-count">
            {total} {total === 1 ? 'file' : 'files'}
            {truncated ? ' (largest listed)' : ''}
          </p>
        </div>
        <label className="sr-only" htmlFor="disk-sort">
          Sort by
        </label>
        <select
          id="disk-sort"
          value={sort}
          onChange={(e) => changeSort(e.target.value as FileSort)}
          data-testid="disk-sort"
          className="h-9 shrink-0 rounded-xl border border-line bg-surface-2 px-2 text-sm"
        >
          <option value="size">Size</option>
          <option value="name">Name</option>
          <option value="modified">Date</option>
        </select>
      </div>

      <ErrorBanner error={error} onRetry={() => void first.run({ scanId, category: filter.category, folder: filter.folder, sort, offset: 0, limit: PAGE })} />

      {note && (
        <p className="m-0 break-words rounded-xl border border-line bg-surface p-2 text-xs" data-testid="disk-note">
          {note}
        </p>
      )}
      {failures.length > 0 && (
        <ul className="m-0 list-none space-y-1 p-0 text-xs text-danger" data-testid="disk-failures">
          {failures.slice(0, 5).map((f) => (
            <li key={f} className="break-all">
              {f}
            </li>
          ))}
          {failures.length > 5 && <li>...and {failures.length - 5} more</li>}
        </ul>
      )}

      {rows.length > 0 && (
        <div className="flex items-center gap-2 px-1">
          <Checkbox
            checked={allLoadedSelected}
            indeterminate={selected.size > 0 && !allLoadedSelected}
            onChange={(on) => setSelected(on ? new Set(rows.map((r) => r.path)) : new Set())}
            ariaLabel="Select all listed files"
            testId="disk-select-all"
          />
          <span className="text-xs text-muted">Select all listed</span>
        </div>
      )}

      <ul className="m-0 list-none divide-y divide-line overflow-hidden rounded-xl border border-line bg-surface p-0">
        {rows.map((r) => {
          const { dir } = splitPath(r.path);
          return (
            <li
              key={r.path}
              data-testid={`disk-file-${r.name}`}
              data-path={r.path}
              className="flex min-w-0 items-center gap-2 px-2 py-1.5"
            >
              <Checkbox
                checked={selected.has(r.path)}
                onChange={() => toggle(r.path)}
                ariaLabel={`Select ${r.name}`}
                testId={`disk-check-${r.name}`}
              />
              <span
                className="h-8 w-1 shrink-0 rounded-full"
                style={{ background: `var(--cat-${r.category})` }}
                aria-hidden
              />
              <span className="min-w-0 flex-1">
                <span className="block break-all text-sm">{r.name}</span>
                <span className="block break-all text-[11px] text-muted">{dir}</span>
                <span className="block text-[11px] text-muted">
                  {CATEGORY_LABEL[r.category]} - {formatDate(r.modified)}
                </span>
              </span>
              <span className="shrink-0 text-xs font-medium tabular-nums" data-testid={`disk-size-${r.name}`}>
                {formatBytes(r.bytes)}
              </span>
              <button
                type="button"
                onClick={() => void openFolder(r.path)}
                aria-label={`Show ${r.name} in its folder`}
                data-testid={`disk-open-${r.name}`}
                className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border-0 bg-transparent"
              >
                <FolderOpen size={17} className="text-muted" aria-hidden />
              </button>
            </li>
          );
        })}
      </ul>

      {loading && (
        <p className="m-0 flex items-center justify-center gap-2 py-3 text-sm text-muted" data-testid="disk-files-loading">
          <Loader2 size={15} className="animate-spin" aria-hidden /> Loading...
        </p>
      )}
      {!loading && rows.length === 0 && !error && (
        <p className="m-0 py-4 text-center text-sm text-muted" data-testid="disk-files-empty">
          No files here.
        </p>
      )}
      {!loading && rows.length < total && (
        <button
          type="button"
          onClick={() => void loadMore()}
          data-testid="disk-load-more"
          className="h-9 rounded-xl border border-line bg-surface-2 text-sm font-medium"
        >
          Load more ({total - rows.length} left)
        </button>
      )}

      {selected.size > 0 && (
        <div className="sticky bottom-0 z-10 rounded-xl border border-line bg-surface p-2 shadow-lg">
          <button
            type="button"
            onClick={() => setConfirm(true)}
            disabled={busy}
            data-testid="btn-disk-delete"
            className="flex h-10 w-full items-center justify-center gap-1.5 rounded-xl border-0 bg-danger text-sm font-semibold text-white disabled:opacity-50"
          >
            {busy ? <Loader2 size={16} className="animate-spin" aria-hidden /> : <Trash2 size={16} aria-hidden />}
            Delete {selected.size} {selected.size === 1 ? 'file' : 'files'} ({formatBytes(selectedBytes)})
          </button>
        </div>
      )}

      <ConfirmSheet
        open={confirm}
        title={`Delete ${selected.size} ${selected.size === 1 ? 'file' : 'files'}?`}
        message={`${formatBytes(selectedBytes)} will be permanently deleted. This cannot be undone.`}
        confirmLabel="Delete"
        danger
        onConfirm={() => void doDelete()}
        onCancel={() => setConfirm(false)}
      />
    </div>
  );
}
