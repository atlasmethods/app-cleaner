/** Planned methods for the `cookies` feature. Request/response types are added when it is implemented. */
export const COOKIES_METHODS = [
  'cookies.list',
  'cookies.get_keep_list',
  'cookies.set_keep_list',
] as const;

export type CookiesMethod = (typeof COOKIES_METHODS)[number];
