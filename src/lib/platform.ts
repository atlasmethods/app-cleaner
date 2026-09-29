/** True when the UI runs on Windows (the browser/webview runs on the same machine as the backend). */
export function isWindowsClient(nav: Pick<Navigator, 'userAgent'> | undefined = globalThis.navigator): boolean {
  return !!nav && /windows/i.test(nav.userAgent);
}
