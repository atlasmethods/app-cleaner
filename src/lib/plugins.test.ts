import { describe, expect, it } from 'vitest';
import type { Plugin } from '../api/browser_plugins';
import { closeBrowserName, filterPlugins, groupPlugins, profileTitle } from './plugins';

const plugin = (over: Partial<Plugin> & { id: string }): Plugin => ({
  browser: 'chrome',
  browserLabel: 'Google Chrome',
  profile: 'Default',
  extensionId: over.id,
  name: over.id,
  version: '1',
  description: '',
  enabled: true,
  type: 'extension',
  installLocation: '/x',
  canDisable: true,
  canRemove: true,
  running: false,
  ...over,
});

describe('plugin helpers', () => {
  const list = [
    plugin({ id: 'a', name: 'Ad Blocker' }),
    plugin({ id: 'b', profile: 'Profile 1', profileName: 'Work' }),
    plugin({ id: 'c', browser: 'firefox', browserLabel: 'Firefox', profile: 'x.default', running: true }),
    plugin({ id: 'd', name: 'Dark theme', description: 'Night look', type: 'theme' }),
  ];

  it('groups by browser then profile in the given order', () => {
    const g = groupPlugins(list);
    expect(g.map((b) => b.browserLabel)).toEqual(['Google Chrome', 'Firefox']);
    expect(g[0]!.profiles.map((p) => [p.profile, p.plugins.map((x) => x.id)])).toEqual([
      ['Default', ['a', 'd']],
      ['Profile 1', ['b']],
    ]);
    expect(g[0]!.running).toBe(false);
    expect(g[1]!.running).toBe(true);
    expect(groupPlugins([])).toEqual([]);
  });

  it('searches name, description and id', () => {
    expect(filterPlugins(list, 'ad blo').map((p) => p.id)).toEqual(['a']);
    expect(filterPlugins(list, 'NIGHT').map((p) => p.id)).toEqual(['d']);
    expect(filterPlugins(list, ' ')).toHaveLength(4);
    expect(filterPlugins(list, 'zzz')).toEqual([]);
  });

  it('recognises the "close the browser first" error', () => {
    expect(closeBrowserName('Close Google Chrome first: while it is running it would undo this change.')).toBe('Google Chrome');
    expect(closeBrowserName('Close Firefox first')).toBe('Firefox');
    expect(closeBrowserName('permission denied')).toBeNull();
  });

  it('titles profiles', () => {
    expect(profileTitle({ profile: 'Profile 1', profileName: 'Work', plugins: [] })).toBe('Work (Profile 1)');
    expect(profileTitle({ profile: 'Default', plugins: [] })).toBe('Default');
    expect(profileTitle({ profile: 'Default', profileName: 'Default', plugins: [] })).toBe('Default');
  });
});
