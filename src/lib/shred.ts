import type { PathResult } from '../api/secure_delete';

/** The word the user must type before anything is shredded. */
export const SHRED_WORD = 'SHRED';

/** Absolute on any platform: `/x`, `C:\x`, `C:/x` or a UNC path `\\host\share`. */
export function isAbsolutePath(p: string): boolean {
  return p.startsWith('/') || /^[A-Za-z]:[\\/]/.test(p) || p.startsWith('\\\\');
}

/** Why `p` cannot be shredded, or null when it looks fine. Protected paths are refused by the backend. */
export function pathProblem(p: string): string | null {
  if (!p) return 'Enter a path';
  if (!isAbsolutePath(p)) return 'Use an absolute path (starting with / or a drive letter)';
  if (/[\0\r\n]/.test(p)) return 'The path contains a control character';
  return null;
}

/** Split pasted text into paths: one per line, quotes and blank lines dropped, duplicates removed. */
export function parsePaths(text: string): string[] {
  const out: string[] = [];
  for (const raw of text.split(/\r?\n/)) {
    let p = raw.trim();
    if (p.length > 1 && ((p.startsWith('"') && p.endsWith('"')) || (p.startsWith("'") && p.endsWith("'")))) {
      p = p.slice(1, -1).trim();
    }
    if (p && !out.includes(p)) out.push(p);
  }
  return out;
}

export interface ShredSummary {
  done: number;
  failed: number;
  bytes: number;
}

export function summarize(results: PathResult[]): ShredSummary {
  const done = results.filter((r) => r.ok).length;
  return { done, failed: results.length - done, bytes: results.filter((r) => r.ok).reduce((n, r) => n + r.bytes, 0) };
}
