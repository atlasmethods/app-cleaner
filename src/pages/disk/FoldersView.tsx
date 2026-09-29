import { ChevronRight, Files } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import type { TreeNode } from '../../api/disk_analyzer';
import { ErrorBanner } from '../../components/ErrorBanner';
import { breadcrumbs, folderBars } from '../../lib/diskView';
import { formatBytes } from '../../lib/format';
import { useCall } from '../../hooks/useCall';

interface Props {
  scanId: string;
  roots: string[];
  /** Bump to reload the current folder (after a delete). */
  version: number;
  onShowFiles: (folder: string) => void;
}

/** Breadcrumb + one horizontal bar per sub-folder, biggest first. */
export function FoldersView({ scanId, roots, version, onShowFiles }: Props) {
  const tree = useCall<TreeNode, { scanId: string; path?: string }>('disk_analyzer.tree');
  // `undefined` / `null` = the scan's top level.
  const [at, setAt] = useState<string | null | undefined>(undefined);

  const run = tree.run;
  useEffect(() => {
    void run(at ? { scanId, path: at } : { scanId });
  }, [run, scanId, at, version]);

  const node = tree.data;
  const error = tree.error;
  const retry = () => void run(at ? { scanId, path: at } : { scanId });

  const bars = useMemo(() => (node ? folderBars(node.children, node.bytes) : []), [node]);
  const crumbs = useMemo(() => (node ? breadcrumbs(node.path, roots) : []), [node, roots]);

  return (
    <div className="flex min-w-0 flex-col gap-2" data-testid="disk-folders">
      <ErrorBanner error={error} onRetry={retry} />
      {node && (
        <>
          <nav aria-label="Folder path" className="flex min-w-0 flex-wrap items-center gap-x-1 text-xs" data-testid="disk-crumbs">
            {crumbs.map((c, i) => (
              <span key={`${c.path}-${i}`} className="flex min-w-0 items-center gap-1">
                {i > 0 && <ChevronRight size={12} className="shrink-0 text-muted" aria-hidden />}
                {i === crumbs.length - 1 ? (
                  <span className="break-all font-semibold" aria-current="page">
                    {c.label}
                  </span>
                ) : (
                  <button
                    type="button"
                    onClick={() => setAt(c.path)}
                    data-testid={`disk-crumb-${i}`}
                    className="break-all border-0 bg-transparent p-0 text-accent underline"
                  >
                    {c.label}
                  </button>
                )}
              </span>
            ))}
          </nav>
          <p className="m-0 text-xs text-muted" data-testid="disk-folder-total">
            {formatBytes(node.bytes)} in {node.files} {node.files === 1 ? 'file' : 'files'}
          </p>

          <ul className="m-0 list-none divide-y divide-line overflow-hidden rounded-xl border border-line bg-surface p-0">
            {node.ownFiles > 0 && node.path !== null && (
              <li>
                <button
                  type="button"
                  onClick={() => onShowFiles(node.path as string)}
                  data-testid="disk-folder-files"
                  className="flex min-h-11 w-full min-w-0 items-center gap-2 border-0 bg-transparent px-3 py-2 text-left"
                >
                  <Files size={16} className="shrink-0 text-muted" aria-hidden />
                  <span className="min-w-0 flex-1 text-sm">
                    Files in this folder
                    <span className="block text-[11px] text-muted">
                      {node.ownFiles} here, tap to list this folder&apos;s files
                    </span>
                  </span>
                  <span className="shrink-0 text-xs font-medium tabular-nums">{formatBytes(node.ownBytes)}</span>
                </button>
              </li>
            )}
            {bars.map((b) => (
              <li key={b.path}>
                <button
                  type="button"
                  onClick={() => setAt(b.path)}
                  data-testid={`disk-folder-${b.name}`}
                  className="block min-h-11 w-full min-w-0 border-0 bg-transparent px-3 py-2 text-left"
                >
                  <span className="flex min-w-0 items-baseline gap-2">
                    <span className="min-w-0 flex-1 break-all text-sm">{b.name}</span>
                    <span className="shrink-0 text-xs font-medium tabular-nums">{formatBytes(b.bytes)}</span>
                    <span className="w-10 shrink-0 text-right text-[11px] tabular-nums text-muted">{b.percent}%</span>
                  </span>
                  <span className="mt-1 block h-2 overflow-hidden rounded-full bg-surface-2">
                    <span
                      className="block h-full rounded-full bg-accent"
                      style={{ width: `${b.width}%` }}
                      data-testid={`disk-folder-bar-${b.name}`}
                    />
                  </span>
                </button>
              </li>
            ))}
            {bars.length === 0 && node.ownFiles === 0 && (
              <li className="p-3 text-sm text-muted" data-testid="disk-folder-empty">
                Nothing here.
              </li>
            )}
          </ul>
          {node.childCount > node.children.length && (
            <p className="m-0 text-xs text-muted">Showing the largest {node.children.length} of {node.childCount} folders.</p>
          )}
          {node.canGoUp && (
            <button
              type="button"
              onClick={() => setAt(node.parent)}
              data-testid="disk-folder-up"
              className="h-9 rounded-xl border border-line bg-surface-2 text-sm font-medium"
            >
              Up one level
            </button>
          )}
        </>
      )}
    </div>
  );
}
