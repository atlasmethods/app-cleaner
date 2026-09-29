import { HardDrive, Loader2, PieChart, Plus, Search, X } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import type {
  CategoryTotals,
  DeleteFilesResult,
  DriveInfo,
  FileCategory,
  ScanParams,
  ScanSummary,
} from '../api/disk_analyzer';
import { Card } from '../components/Card';
import { EmptyState } from '../components/EmptyState';
import { ErrorBanner } from '../components/ErrorBanner';
import { RunProgress } from '../components/RunProgress';
import { useCall } from '../hooks/useCall';
import { CATEGORY_LABEL, categoryBars } from '../lib/diskView';
import { formatBytes } from '../lib/format';
import { FileList, type FileFilter } from './disk/FileList';
import { FoldersView } from './disk/FoldersView';
import '../styles/diskCategories.css';

type Tab = 'types' | 'folders';

interface Live {
  totals: CategoryTotals;
  totalFiles: number;
  totalBytes: number;
}

export default function DiskAnalyzerPage() {
  const drivesCall = useCall<DriveInfo[]>('disk_analyzer.list_drives');
  const scan = useCall<ScanSummary, ScanParams>('disk_analyzer.scan');
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [folders, setFolders] = useState<string[]>([]);
  const [folderInput, setFolderInput] = useState('');
  const [tab, setTab] = useState<Tab>('types');
  const [files, setFiles] = useState<{ filter: FileFilter; title: string; from: Tab } | null>(null);
  // Totals after deletions, newer than the scan's own.
  const [live, setLive] = useState<Live | null>(null);
  const [treeVersion, setTreeVersion] = useState(0);

  const runDrives = drivesCall.run;
  useEffect(() => {
    void runDrives();
  }, [runDrives]);

  const drives = drivesCall.data ?? [];
  const paths = useMemo(() => [...picked, ...folders], [picked, folders]);
  const summary = scan.data;
  const totals = live?.totals ?? summary?.totals;
  const totalBytes = live?.totalBytes ?? summary?.totalBytes ?? 0;
  const totalFiles = live?.totalFiles ?? summary?.totalFiles ?? 0;
  const bars = useMemo(() => (totals ? categoryBars(totals) : []), [totals]);

  const analyze = async () => {
    setFiles(null);
    setLive(null);
    setTab('types');
    await scan.run({ paths });
  };

  const addFolder = () => {
    const p = folderInput.trim();
    if (!p || folders.includes(p)) return;
    setFolders((f) => [...f, p]);
    setFolderInput('');
  };

  const togglePick = (mount: string) =>
    setPicked((s) => {
      const n = new Set(s);
      if (n.has(mount)) n.delete(mount);
      else n.add(mount);
      return n;
    });

  const onDeleted = (r: DeleteFilesResult) => {
    setLive({ totals: r.totals, totalFiles: r.totalFiles, totalBytes: r.totalBytes });
    setTreeVersion((v) => v + 1);
  };

  const openCategory = (c: FileCategory) =>
    setFiles({ filter: { category: c }, title: CATEGORY_LABEL[c], from: 'types' });

  return (
    <div data-testid="page-disk" className="flex flex-col gap-3 p-4">
      <ErrorBanner error={drivesCall.error} onRetry={() => void runDrives()} />
      <ErrorBanner error={scan.error} />

      <Card title="What to analyze">
        <div className="flex flex-wrap gap-2" data-testid="disk-drives">
          {drivesCall.loading && drives.length === 0 && (
            <span className="text-sm text-muted">Looking for drives...</span>
          )}
          {drives.map((d) => {
            const on = picked.has(d.mount);
            return (
              <button
                key={d.mount}
                type="button"
                aria-pressed={on}
                onClick={() => togglePick(d.mount)}
                disabled={scan.loading}
                data-testid={`disk-drive-${d.mount}`}
                className={`flex min-h-10 max-w-full min-w-0 items-center gap-1.5 rounded-full border px-3 py-1 text-left text-sm ${
                  on ? 'border-accent bg-accent/15 font-semibold' : 'border-line bg-surface-2'
                }`}
              >
                <HardDrive size={14} className="shrink-0" aria-hidden />
                <span className="min-w-0 break-all">
                  {d.mount}
                  <span className="block text-[11px] font-normal text-muted">
                    {formatBytes(d.total - d.available)} of {formatBytes(d.total)} used
                  </span>
                </span>
              </button>
            );
          })}
          {folders.map((f) => (
            <button
              key={f}
              type="button"
              onClick={() => setFolders((x) => x.filter((y) => y !== f))}
              disabled={scan.loading}
              aria-label={`Remove folder ${f}`}
              data-testid={`disk-folder-chip-${f}`}
              className="flex min-h-10 max-w-full min-w-0 items-center gap-1.5 rounded-full border border-accent bg-accent/15 px-3 py-1 text-left text-sm font-semibold"
            >
              <span className="min-w-0 break-all">{f}</span>
              <X size={14} className="shrink-0" aria-hidden />
            </button>
          ))}
        </div>
        <form
          className="mt-2 flex gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            addFolder();
          }}
        >
          <input
            type="text"
            value={folderInput}
            onChange={(e) => setFolderInput(e.target.value)}
            placeholder="Or type a folder path"
            aria-label="Folder path"
            data-testid="disk-folder-input"
            className="h-10 min-w-0 flex-1 rounded-xl border border-line bg-surface-2 px-3 text-sm"
          />
          <button
            type="submit"
            disabled={!folderInput.trim() || scan.loading}
            aria-label="Add folder"
            data-testid="disk-folder-add"
            className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-line bg-surface-2 disabled:opacity-50"
          >
            <Plus size={16} aria-hidden />
          </button>
        </form>
        <button
          type="button"
          onClick={() => void analyze()}
          disabled={paths.length === 0 || scan.loading}
          data-testid="btn-disk-analyze"
          className="mt-2 flex h-10 w-full items-center justify-center gap-1.5 rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg disabled:opacity-50"
        >
          {scan.loading ? <Loader2 size={16} className="animate-spin" aria-hidden /> : <Search size={16} aria-hidden />}
          Analyze
        </button>
      </Card>

      {scan.loading && (
        <RunProgress label="Analyzing" progress={scan.progress} onCancel={scan.cancel} testId="disk-progress" />
      )}

      {summary && !scan.loading && (
        <>
          <p className="m-0 text-xs text-muted" data-testid="disk-summary">
            {totalFiles} {totalFiles === 1 ? 'file' : 'files'}, {formatBytes(totalBytes)} in {(summary.durationMs / 1000).toFixed(1)}s
            {summary.errors.count > 0 && ` - ${summary.errors.count} folders could not be read`}
          </p>
          {summary.truncated && (
            <p className="m-0 text-xs text-warn" data-testid="disk-truncated">
              Totals cover every file; the file lists show the largest {summary.stored.toLocaleString()}.
            </p>
          )}

          {files ? (
            <FileList
              key={`${files.filter.category ?? ''}|${files.filter.folder ?? ''}`}
              scanId={summary.scanId}
              filter={files.filter}
              title={files.title}
              onBack={() => {
                setTab(files.from);
                setFiles(null);
              }}
              onDeleted={onDeleted}
            />
          ) : (
            <>
              <div role="tablist" aria-label="View" className="flex gap-1 rounded-xl bg-surface-2 p-1">
                {(['types', 'folders'] as const).map((t) => (
                  <button
                    key={t}
                    type="button"
                    role="tab"
                    aria-selected={tab === t}
                    onClick={() => setTab(t)}
                    data-testid={`disk-tab-${t}`}
                    className={`h-8 min-w-0 flex-1 rounded-lg border-0 text-sm font-medium ${
                      tab === t ? 'bg-surface shadow-sm' : 'bg-transparent text-muted'
                    }`}
                  >
                    {t === 'types' ? 'File types' : 'Folders'}
                  </button>
                ))}
              </div>

              {tab === 'types' && (
                <ul className="m-0 list-none divide-y divide-line overflow-hidden rounded-xl border border-line bg-surface p-0" data-testid="disk-categories">
                  {bars.map((b) => (
                    <li key={b.category}>
                      <button
                        type="button"
                        onClick={() => openCategory(b.category)}
                        data-testid={`disk-cat-${b.category}`}
                        className="block min-h-11 w-full min-w-0 border-0 bg-transparent px-3 py-2 text-left"
                      >
                        <span className="flex min-w-0 items-baseline gap-2">
                          <span
                            className="mt-1 h-3 w-3 shrink-0 self-center rounded-sm"
                            style={{ background: `var(--cat-${b.category})` }}
                            aria-hidden
                          />
                          <span className="min-w-0 flex-1 text-sm font-medium">
                            {b.label}
                            <span className="ml-1.5 text-[11px] font-normal text-muted" data-testid={`disk-cat-${b.category}-files`}>
                              {b.files} {b.files === 1 ? 'file' : 'files'}
                            </span>
                          </span>
                          <span className="shrink-0 text-xs font-medium tabular-nums" data-testid={`disk-cat-${b.category}-bytes`}>
                            {formatBytes(b.bytes)}
                          </span>
                          <span className="w-11 shrink-0 text-right text-[11px] tabular-nums text-muted" data-testid={`disk-cat-${b.category}-pct`}>
                            {b.percent}%
                          </span>
                        </span>
                        <span className="mt-1 block h-2 overflow-hidden rounded-full bg-surface-2">
                          <span
                            className="block h-full rounded-full"
                            style={{ width: `${Math.max(b.percent, b.bytes > 0 ? 1 : 0)}%`, background: `var(--cat-${b.category})` }}
                          />
                        </span>
                      </button>
                    </li>
                  ))}
                  {bars.length === 0 && (
                    <li className="p-3 text-sm text-muted" data-testid="disk-empty">
                      No files found.
                    </li>
                  )}
                </ul>
              )}

              {tab === 'folders' && (
                <FoldersView
                  scanId={summary.scanId}
                  roots={summary.roots}
                  version={treeVersion}
                  onShowFiles={(folder) => setFiles({ filter: { folder }, title: folder, from: 'folders' })}
                />
              )}
            </>
          )}
        </>
      )}

      {!summary && !scan.loading && !scan.error && (
        <EmptyState
          icon={PieChart}
          title="See what uses your disk space"
          hint="Pick a drive or type a folder, then tap Analyze."
        />
      )}
    </div>
  );
}
