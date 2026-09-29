import { afterEach, describe, expect, it } from 'vitest';
import { applyTheme, isTheme } from './theme';

describe('theme', () => {
  afterEach(() => {
    document.documentElement.removeAttribute('data-theme');
    window.localStorage.clear();
  });

  it('sets and clears data-theme on <html> and remembers the choice', () => {
    applyTheme('dark');
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
    expect(window.localStorage.getItem('clearsweep.theme')).toBe('dark');
    applyTheme('light');
    expect(document.documentElement.getAttribute('data-theme')).toBe('light');
    applyTheme('system');
    expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
  });

  it('validates theme names', () => {
    expect(isTheme('dark')).toBe(true);
    expect(isTheme('purple')).toBe(false);
    expect(isTheme(undefined)).toBe(false);
  });
});
