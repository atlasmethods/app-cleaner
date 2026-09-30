import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { _resetLayoutForTests, getCompactMode, setCompactMode, useWideLayout, WIDE_QUERY } from './layout';

function stubMedia(initial: boolean) {
  const listeners = new Set<() => void>();
  const state = { matches: initial };
  vi.stubGlobal('matchMedia', (q: string) => ({
    get matches() {
      return q === WIDE_QUERY && state.matches;
    },
    addEventListener: (_: string, cb: () => void) => listeners.add(cb),
    removeEventListener: (_: string, cb: () => void) => listeners.delete(cb),
  }));
  return {
    set: (v: boolean) => {
      state.matches = v;
      listeners.forEach((l) => l());
    },
  };
}

describe('useWideLayout', () => {
  beforeEach(() => {
    window.localStorage.clear();
    _resetLayoutForTests();
  });
  afterEach(() => vi.unstubAllGlobals());

  it('is wide from 900px and follows the window', () => {
    expect(WIDE_QUERY).toBe('(min-width: 900px)');
    const mq = stubMedia(false);
    const { result } = renderHook(() => useWideLayout());
    expect(result.current).toBe(false);
    act(() => mq.set(true));
    expect(result.current).toBe(true);
    act(() => mq.set(false));
    expect(result.current).toBe(false);
  });

  it('compact mode forces the compact layout and is remembered', () => {
    stubMedia(true);
    const { result } = renderHook(() => useWideLayout());
    expect(result.current).toBe(true);
    act(() => setCompactMode(true));
    expect(result.current).toBe(false);
    expect(getCompactMode()).toBe(true);
    expect(window.localStorage.getItem('clearsweep.compact')).toBe('1');
    act(() => setCompactMode(false));
    expect(result.current).toBe(true);
    expect(window.localStorage.getItem('clearsweep.compact')).toBe('0');
  });

  it('is compact where matchMedia does not exist', () => {
    vi.stubGlobal('matchMedia', undefined);
    const { result } = renderHook(() => useWideLayout());
    expect(result.current).toBe(false);
  });
});
