import { describe, expect, it } from 'vitest';
import type { CookieDomain } from '../api/cookies';
import {
  addKeep,
  buildLists,
  hostMatchesDomain,
  isKept,
  normalizeDomain,
  removeKeep,
  selectedVisible,
  toggleSelected,
} from './cookieLists';

const c = (domain: string, count = 1, kept = false): CookieDomain => ({ domain, count, browsers: ['Chrome'], kept });

describe('domain matching', () => {
  it('matches the domain, its subdomains, and nothing that merely ends alike', () => {
    expect(hostMatchesDomain('google.com', 'google.com')).toBe(true);
    expect(hostMatchesDomain('accounts.google.com', 'google.com')).toBe(true);
    expect(hostMatchesDomain('notgoogle.com', 'google.com')).toBe(false);
    expect(hostMatchesDomain('google.com.evil.io', 'google.com')).toBe(false);
    expect(hostMatchesDomain('192.168.1.1', '168.1.1')).toBe(false);
    expect(hostMatchesDomain('192.168.1.1', '192.168.1.1')).toBe(true);
    expect(isKept('a.b.example.org', ['x.com', 'example.org'])).toBe(true);
    expect(isKept('example.org', [])).toBe(false);
  });

  it('normalizes typed domains like the server does', () => {
    expect(normalizeDomain(' .GitHub.com ')).toBe('github.com');
    expect(normalizeDomain('https://Example.org/path?q=1')).toBe('example.org');
    expect(normalizeDomain('*.example.org')).toBe('example.org');
    expect(normalizeDomain('example.com:8080')).toBe('example.com');
    for (const bad of ['', '.', 'a b.com', 'a..b', '50%.com']) expect(normalizeDomain(bad)).toBeNull();
  });
});

describe('moving between lists', () => {
  const cookies = [c('google.com', 2), c('accounts.google.com', 1), c('notgoogle.com'), c('github.com', 3), c('tracker.example', 4)];

  it('puts unkept domains on the left and the keep list on the right', () => {
    const l = buildLists(cookies, ['google.com'], '');
    expect(l.all.map((x) => x.domain)).toEqual(['github.com', 'notgoogle.com', 'tracker.example']);
    expect(l.keep).toEqual([{ domain: 'google.com', count: 3 }]);
  });

  it('keeping a domain moves it (and its subdomains); removing moves it back', () => {
    let keep = addKeep([], 'GitHub.com');
    expect(keep).toEqual(['github.com']);
    expect(buildLists(cookies, keep, '').all.map((x) => x.domain)).not.toContain('github.com');
    keep = addKeep(keep, 'github.com');
    expect(keep).toEqual(['github.com']);
    keep = removeKeep(keep, 'github.com');
    expect(buildLists(cookies, keep, '').all.map((x) => x.domain)).toContain('github.com');
  });

  it('ignores invalid input and keeps entries without cookies', () => {
    expect(addKeep(['a.com'], 'not valid')).toEqual(['a.com']);
    const l = buildLists([], ['nothing.here'], '');
    expect(l.keep).toEqual([{ domain: 'nothing.here', count: 0 }]);
  });

  it('search filters both lists, case-insensitively', () => {
    const l = buildLists(cookies, ['google.com'], ' GOOG');
    expect(l.all.map((x) => x.domain)).toEqual(['notgoogle.com']);
    expect(l.keep.map((x) => x.domain)).toEqual(['google.com']);
    expect(buildLists(cookies, [], 'zzz').all).toEqual([]);
  });
});

describe('multi-select', () => {
  it('toggles without mutating', () => {
    const a = new Set<string>();
    const b = toggleSelected(a, 'x.com');
    expect(a.size).toBe(0);
    expect(b.has('x.com')).toBe(true);
    expect(toggleSelected(b, 'x.com').size).toBe(0);
  });

  it('only counts selected domains that are still visible', () => {
    const shown = [c('a.com'), c('b.com')];
    expect(selectedVisible(new Set(['a.com', 'gone.com']), shown)).toEqual(['a.com']);
  });
});
