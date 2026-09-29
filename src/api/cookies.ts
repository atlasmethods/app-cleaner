/** Mirrors `sweep_core::features::cookies` (serde camelCase). */
export const COOKIES_METHODS = [
  'cookies.list',
  'cookies.get_keep_list',
  'cookies.set_keep_list',
  'cookies.intelligent_scan',
  'cookies.delete',
] as const;

export type CookiesMethod = (typeof COOKIES_METHODS)[number];

export interface CookieDomain {
  /** Normalized: lower case, no leading dot. */
  domain: string;
  count: number;
  browsers: string[];
  kept: boolean;
}

export interface KeepList {
  domains: string[];
}

export interface ScanResult extends KeepList {
  added: string[];
}

export interface BrowserDelete {
  browser: string;
  deleted: number;
  skipped?: 'app_running' | 'in_use';
  errors: string[];
}

export interface DeleteResult {
  deleted: number;
  browsers: BrowserDelete[];
}
