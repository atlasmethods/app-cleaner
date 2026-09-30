import { afterEach, describe, expect, it } from 'vitest';
import en from './en.json';
import { CATALOGS, LANGUAGES, getLanguage, setLanguage, t } from './index';

afterEach(() => setLanguage('en'));

describe('t()', () => {
  it('returns the English text', () => {
    expect(t('tab.home')).toBe('Home');
    expect(t('settings.smart')).toBe('Smart Cleaning');
  });

  it('falls back to the key for unknown keys', () => {
    expect(t('no.such.key')).toBe('no.such.key');
  });

  it('falls back to English for a language that lacks a key, and for unknown languages', () => {
    CATALOGS.xx = { 'tab.home': 'Casa' };
    setLanguage('xx');
    expect(getLanguage()).toBe('xx');
    expect(t('tab.home')).toBe('Casa');
    expect(t('tab.clean')).toBe('Clean');
    delete CATALOGS.xx;
    setLanguage('zz');
    expect(getLanguage()).toBe('en');
  });

  it('has no empty texts and offers exactly the catalogs it has', () => {
    for (const v of Object.values(en)) expect(v.trim().length).toBeGreaterThan(0);
    expect(LANGUAGES.map((l) => l.code).sort()).toEqual(Object.keys(CATALOGS).sort());
  });
});
