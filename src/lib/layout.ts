import { useSyncExternalStore } from 'react';

/** Width (px) from which the two-pane layout (navigation rail + content column) is used. */
export const WIDE_MIN_WIDTH = 900;
export const WIDE_QUERY = `(min-width: ${WIDE_MIN_WIDTH}px)`;

const KEY = 'clearsweep.compact';

function readCache(): boolean {
  try {
    return window.localStorage.getItem(KEY) === '1';
  } catch {
    return false;
  }
}

let compact = typeof window !== 'undefined' ? readCache() : false;
const listeners = new Set<() => void>();

/** True when the user forces the compact (phone-width, bottom tab bar) layout on wide windows. */
export function getCompactMode(): boolean {
  return compact;
}

/** Apply (and remember for the next start-up) the compact-mode preference. */
export function setCompactMode(value: boolean): void {
  try {
    window.localStorage.setItem(KEY, value ? '1' : '0');
  } catch {
    /* storage unavailable: the server-side setting still wins on next start */
  }
  if (value === compact) return;
  compact = value;
  listeners.forEach((l) => l());
}

function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  const mq = typeof window.matchMedia === 'function' ? window.matchMedia(WIDE_QUERY) : null;
  mq?.addEventListener('change', cb);
  return () => {
    listeners.delete(cb);
    mq?.removeEventListener('change', cb);
  };
}

function snapshot(): boolean {
  if (compact || typeof window.matchMedia !== 'function') return false;
  return window.matchMedia(WIDE_QUERY).matches;
}

/** Two-pane layout in use: the window is at least 900px wide and compact mode is not forced. */
export function useWideLayout(): boolean {
  return useSyncExternalStore(subscribe, snapshot, () => false);
}

/** Test helper. */
export function _resetLayoutForTests(): void {
  compact = false;
  listeners.clear();
}
