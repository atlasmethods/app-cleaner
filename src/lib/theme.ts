import type { Settings, Theme } from '../api/settings';
import { call } from './transport';

const KEY = 'clearsweep.theme';

export function isTheme(v: unknown): v is Theme {
  return v === 'system' || v === 'light' || v === 'dark';
}

/** "system" removes the override so the OS preference (prefers-color-scheme) applies. */
export function applyTheme(theme: Theme): void {
  const el = document.documentElement;
  if (theme === 'system') el.removeAttribute('data-theme');
  else el.setAttribute('data-theme', theme);
  try {
    window.localStorage.setItem(KEY, theme);
  } catch {
    /* storage unavailable: the server-side setting still wins on next start */
  }
}

/**
 * Apply the last known theme synchronously (no flash), then confirm it against the
 * persisted setting. Never throws: theming must not break app start-up.
 */
export function initTheme(): void {
  try {
    const cached = window.localStorage.getItem(KEY);
    if (isTheme(cached)) applyTheme(cached);
  } catch {
    /* ignore */
  }
  void call<Settings>('settings.get')
    .then((s) => {
      if (isTheme(s.theme)) applyTheme(s.theme);
    })
    .catch(() => undefined);
}
