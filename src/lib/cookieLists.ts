import type { CookieDomain } from '../api/cookies';

/** Mirrors the server's `normalize_domain` for what people type or paste. */
export function normalizeDomain(input: string): string | null {
  let s = input.trim().toLowerCase();
  const scheme = s.indexOf('://');
  if (scheme >= 0) s = s.slice(scheme + 3);
  s = s.split(/[/?#]/)[0] ?? '';
  s = s.replace(/:\d+$/, '');
  s = s.replace(/^\*\./, '').replace(/^\.+|\.+$/g, '');
  if (!s || s.length > 253 || s.includes('..')) return null;
  if (/[\s%*,;"'<>@:\\]/.test(s)) return null;
  return s;
}

/** Equal to `domain`, or a subdomain of it. Numeric (IPv4) hosts only match exactly. */
export function hostMatchesDomain(host: string, domain: string): boolean {
  if (host === domain) return true;
  if (/^[\d.]+$/.test(host)) return false;
  return host.length > domain.length && host.endsWith(`.${domain}`);
}

export function isKept(host: string, keep: readonly string[]): boolean {
  return keep.some((k) => hostMatchesDomain(host, k));
}

export interface KeepRow {
  domain: string;
  /** Cookies currently stored under this domain (including subdomains). */
  count: number;
}

export interface Lists {
  /** Domains with cookies that are not covered by the keep list. */
  all: CookieDomain[];
  /** The keep list, with how many cookies each entry protects right now. */
  keep: KeepRow[];
}

function matches(query: string, domain: string): boolean {
  const q = query.trim().toLowerCase();
  return q === '' || domain.includes(q);
}

/** Split cookies into "on this computer" and "to keep", filtered by `query`. */
export function buildLists(cookies: CookieDomain[], keep: readonly string[], query: string): Lists {
  const all = cookies
    .filter((c) => !isKept(c.domain, keep))
    .filter((c) => matches(query, c.domain))
    .sort((a, b) => a.domain.localeCompare(b.domain));
  const rows = keep
    .filter((k) => matches(query, k))
    .map((k) => ({
      domain: k,
      count: cookies.filter((c) => hostMatchesDomain(c.domain, k)).reduce((n, c) => n + c.count, 0),
    }))
    .sort((a, b) => a.domain.localeCompare(b.domain));
  return { all, keep: rows };
}

/** Add a domain, ignoring duplicates and invalid input. */
export function addKeep(keep: readonly string[], domain: string): string[] {
  const d = normalizeDomain(domain);
  if (!d || keep.includes(d)) return [...keep];
  return [...keep, d];
}

export function removeKeep(keep: readonly string[], domain: string): string[] {
  return keep.filter((k) => k !== domain);
}

/** Selection helpers for the multi-select delete. */
export function toggleSelected(sel: ReadonlySet<string>, domain: string): Set<string> {
  const next = new Set(sel);
  if (next.has(domain)) next.delete(domain);
  else next.add(domain);
  return next;
}

/** Selected domains that are still listed (a search or a move may have hidden some). */
export function selectedVisible(sel: ReadonlySet<string>, shown: readonly CookieDomain[]): string[] {
  const visible = new Set(shown.map((c) => c.domain));
  return [...sel].filter((d) => visible.has(d));
}
