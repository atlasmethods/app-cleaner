const UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];

/** 1536 -> "1.5 KB" (binary units, shown with the usual short names). */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—';
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024;
    i++;
  }
  return `${i === 0 ? v : v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${UNITS[i]}`;
}

/** 93784 -> "1d 2h 3m" */
export function formatDuration(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) return '—';
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `${d}d ${h}h ${m}m`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m ${Math.floor(secs % 60)}s`;
}

export function percent(part: number, whole: number): number {
  return whole > 0 ? Math.min(100, Math.max(0, (part / whole) * 100)) : 0;
}

/** "3 files, 12 entries" (or the actions that ran when nothing was counted). */
export function describeCounts(it: { files: number; rows: number; actions?: string[] }): string {
  const parts: string[] = [];
  if (it.files > 0) parts.push(`${it.files} ${it.files === 1 ? 'file' : 'files'}`);
  if (it.rows > 0) parts.push(`${it.rows} ${it.rows === 1 ? 'entry' : 'entries'}`);
  if (parts.length === 0 && it.actions && it.actions.length > 0) parts.push(it.actions.join(', '));
  return parts.join(', ');
}

/** Local, human readable time for an RFC 3339 string; falls back to the input. */
export function formatWhen(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}
