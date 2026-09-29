import type { CategoryTotals, FileCategory, TreeChild } from '../api/disk_analyzer';
import { FILE_CATEGORIES } from '../api/disk_analyzer';

export const CATEGORY_LABEL: Record<FileCategory, string> = {
  pictures: 'Pictures',
  music: 'Music',
  documents: 'Documents',
  video: 'Video',
  compressed: 'Compressed',
  email: 'Email',
  other: 'Other',
};

export interface CategoryBar {
  category: FileCategory;
  label: string;
  files: number;
  bytes: number;
  /** Share of all analyzed bytes, 0..100 (one decimal). */
  percent: number;
}

export function percentOf(part: number, whole: number): number {
  if (!(whole > 0)) return 0;
  return Math.round(Math.min(100, Math.max(0, (part / whole) * 100)) * 10) / 10;
}

/** Categories that have files, biggest first (ties keep the fixed category order). */
export function categoryBars(totals: CategoryTotals): CategoryBar[] {
  const total = FILE_CATEGORIES.reduce((s, c) => s + totals[c].bytes, 0);
  return FILE_CATEGORIES.map((c, i) => ({ i, c }))
    .filter(({ c }) => totals[c].files > 0)
    .map(({ c, i }) => ({
      i,
      bar: {
        category: c,
        label: CATEGORY_LABEL[c],
        files: totals[c].files,
        bytes: totals[c].bytes,
        percent: percentOf(totals[c].bytes, total),
      } satisfies CategoryBar,
    }))
    .sort((a, b) => b.bar.bytes - a.bar.bytes || a.i - b.i)
    .map((x) => x.bar);
}

export interface FolderBar extends TreeChild {
  /** Share of the parent folder, 0..100. */
  percent: number;
  /** Bar length relative to the biggest sibling, 0..100 (keeps small folders visible). */
  width: number;
}

export function folderBars(children: readonly TreeChild[], parentBytes: number): FolderBar[] {
  const max = children.reduce((m, c) => Math.max(m, c.bytes), 0);
  return children.map((c) => ({
    ...c,
    percent: percentOf(c.bytes, parentBytes),
    width: max > 0 ? Math.max(c.bytes > 0 ? 2 : 0, Math.round((c.bytes / max) * 100)) : 0,
  }));
}

const SEP = /[\\/]+/;

export interface Crumb {
  label: string;
  /** Path to pass to `tree`; `null` = the top level of a multi-folder scan. */
  path: string | null;
}

/**
 * Breadcrumb trail from the scanned root down to `path`. With several roots a leading
 * "All" crumb leads back to the virtual top level. The root crumb shows the full root path.
 */
export function breadcrumbs(path: string | null, roots: readonly string[]): Crumb[] {
  const crumbs: Crumb[] = [];
  if (roots.length > 1) crumbs.push({ label: 'All', path: null });
  if (path === null) return crumbs;
  const root = [...roots]
    .filter((r) => isWithin(path, r))
    .sort((a, b) => b.length - a.length)[0];
  if (!root) return [...crumbs, { label: path, path }];
  crumbs.push({ label: root, path: root });
  const rel = path.slice(root.length).split(SEP).filter(Boolean);
  const sep = root.includes('\\') && !root.includes('/') ? '\\' : '/';
  let cur = root.replace(/[\\/]+$/, '');
  for (const part of rel) {
    cur = `${cur}${sep}${part}`;
    crumbs.push({ label: part, path: cur });
  }
  return crumbs;
}

function isWithin(path: string, root: string): boolean {
  const r = root.replace(/[\\/]+$/, '');
  return path === root || path === r || path.startsWith(`${r}/`) || path.startsWith(`${r}\\`);
}

/** Split `/a/b/c.txt` into folder and file name (handles both separators). */
export function splitPath(path: string): { dir: string; name: string } {
  const i = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  return i < 0 ? { dir: '', name: path } : { dir: path.slice(0, i) || path.slice(0, i + 1), name: path.slice(i + 1) };
}

export function formatDate(unixSeconds: number): string {
  if (!unixSeconds) return '';
  return new Date(unixSeconds * 1000).toLocaleDateString(undefined, { dateStyle: 'medium' });
}
