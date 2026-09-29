import { describe, expect, it } from 'vitest';
import type { AppEntry } from '../api/uninstall';
import { actionsFor, sourceLabel, subtitle, uninstallWarning, visibleApps } from './apps';

const app = (over: Partial<AppEntry> & { name: string }): AppEntry => ({
  id: `dpkg:${over.name}`,
  version: '1.0',
  publisher: '',
  source: 'dpkg',
  uninstallable: true,
  canRepair: false,
  canModify: false,
  isSystem: false,
  ...over,
});

const APPS: AppEntry[] = [
  app({ name: 'zeta', sizeBytes: 300, installDate: '2024-01-01' }),
  app({ name: 'Alpha', sizeBytes: 100, installDate: '2024-03-01', publisher: 'Acme Corp' }),
  app({ name: 'beta', installDate: '2023-01-01' }),
  app({ name: 'bash', isSystem: true, sizeBytes: 9999 }),
  app({ name: 'gamma', sizeBytes: 200 }),
];

describe('visibleApps', () => {
  const names = (o: Partial<Parameters<typeof visibleApps>[1]> = {}) =>
    visibleApps(APPS, { query: '', showSystem: false, sort: 'name', ...o }).map((a) => a.name);

  it('hides system components by default and sorts by name ignoring case', () => {
    expect(names()).toEqual(['Alpha', 'beta', 'gamma', 'zeta']);
    expect(names({ showSystem: true })).toEqual(['Alpha', 'bash', 'beta', 'gamma', 'zeta']);
  });

  it('sorts by size descending with unknown sizes last', () => {
    expect(names({ sort: 'size', showSystem: true })).toEqual(['bash', 'zeta', 'gamma', 'Alpha', 'beta']);
  });

  it('sorts by install date newest first with unknown dates last', () => {
    expect(names({ sort: 'date' })).toEqual(['Alpha', 'zeta', 'beta', 'gamma']);
  });

  it('searches name and publisher case-insensitively and trims', () => {
    expect(names({ query: '  ALP ' })).toEqual(['Alpha']);
    expect(names({ query: 'acme' })).toEqual(['Alpha']);
    expect(names({ query: 'nothing' })).toEqual([]);
    // a system component can be found only when shown
    expect(names({ query: 'bash' })).toEqual([]);
    expect(names({ query: 'bash', showSystem: true })).toEqual(['bash']);
  });

  it('does not mutate its input', () => {
    const copy = [...APPS];
    visibleApps(APPS, { query: '', showSystem: true, sort: 'size' });
    expect(APPS).toEqual(copy);
  });
});

describe('actionsFor', () => {
  it('offers only what the entry supports', () => {
    expect(actionsFor(app({ name: 'a' }))).toEqual({ uninstall: true, repair: false, renameEntry: false, removeEntry: false });
    expect(actionsFor(app({ name: 'a', uninstallable: false }))).toMatchObject({ uninstall: false });
    const win = actionsFor(app({ name: 'w', source: 'windows', canRepair: true }));
    expect(win).toEqual({ uninstall: true, repair: true, renameEntry: true, removeEntry: true });
    // canRepair only counts on Windows
    expect(actionsFor(app({ name: 'x', canRepair: true })).repair).toBe(false);
  });
});

describe('labels', () => {
  it('source labels', () => {
    expect(sourceLabel('dpkg')).toBe('deb');
    expect(sourceLabel('appimage')).toBe('AppImage');
  });
  it('subtitle skips blanks', () => {
    expect(subtitle(app({ name: 'a', version: '1.2', publisher: 'P' }))).toBe('1.2 - P');
    expect(subtitle(app({ name: 'a', version: '', publisher: 'P' }))).toBe('P');
    expect(subtitle(app({ name: 'a', version: ' ', publisher: '' }))).toBe('');
  });
  it('warns strongly for system components', () => {
    expect(uninstallWarning(app({ name: 'bash', isSystem: true }))).toContain('system component');
    expect(uninstallWarning(app({ name: 'x.AppImage', source: 'appimage' }))).toContain('deletes the AppImage');
    expect(uninstallWarning(app({ name: 'Foo', source: 'macapp' }))).toContain('Trash');
  });
});
