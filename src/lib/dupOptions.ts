import type { DupScanParams, MatchBy } from '../api/duplicates';

export interface DupOptions {
  paths: string[];
  exclude: string[];
  match: MatchBy;
  /** Megabytes as typed; empty = no limit. */
  minMb: string;
  maxMb: string;
  hidden: boolean;
  system: boolean;
  skipZero: boolean;
}

export const DEFAULT_OPTIONS: DupOptions = {
  paths: [],
  exclude: [],
  match: { name: false, size: false, modified: false, content: true },
  minMb: '',
  maxMb: '',
  hidden: false,
  system: false,
  skipZero: true,
};

const MB = 1024 * 1024;

/** `''` -> undefined, `'1.5'` -> 1572864; anything else is an error message. */
function parseMb(s: string, label: string): { bytes?: number; error?: string } {
  const t = s.trim();
  if (t === '') return {};
  const n = Number(t);
  if (!Number.isFinite(n) || n < 0) return { error: `${label} must be a number of megabytes, 0 or more` };
  return { bytes: Math.round(n * MB) };
}

export function matchSummary(m: MatchBy): string {
  const parts: string[] = [];
  if (m.content) parts.push('content');
  if (m.name) parts.push('name');
  if (m.size && !m.content) parts.push('size');
  if (m.modified) parts.push('date');
  return parts.length > 0 ? parts.join(' + ') : 'nothing selected';
}

export function anyMatch(m: MatchBy): boolean {
  return m.name || m.size || m.modified || m.content;
}

/** Validate the options and build the `duplicates.scan` parameters. */
export function toScanParams(o: DupOptions): { params: DupScanParams } | { error: string } {
  if (o.paths.length === 0) return { error: 'Add at least one folder to search.' };
  if (!anyMatch(o.match)) return { error: 'Choose at least one way to match files.' };
  const min = parseMb(o.minMb, 'Minimum size');
  if (min.error) return { error: min.error };
  const max = parseMb(o.maxMb, 'Maximum size');
  if (max.error) return { error: max.error };
  if (min.bytes !== undefined && max.bytes !== undefined && min.bytes > max.bytes) {
    return { error: 'Minimum size is larger than the maximum size.' };
  }
  return {
    params: {
      paths: o.paths,
      excludePaths: o.exclude.length > 0 ? o.exclude : undefined,
      matchBy: o.match,
      minSize: min.bytes,
      maxSize: max.bytes,
      includeHidden: o.hidden,
      includeSystem: o.system,
      skipZeroByte: o.skipZero,
      followLinks: false,
    },
  };
}
