import { describe, expect, it } from 'vitest';
import type { StartupItem } from '../api/startup';
import { deleteMessage, hiddenSystemCount, kindsPresent, subline, visibleItems } from './startup';

const item = (over: Partial<StartupItem> & { id: string }): StartupItem => ({
  name: over.id,
  command: '/usr/bin/' + over.id,
  location: '/x',
  kind: 'autostart',
  scope: 'user',
  enabled: true,
  impact: 'unknown',
  canDisable: true,
  canDelete: true,
  critical: false,
  ...over,
});

const ITEMS = [
  item({ id: 'slack', name: 'Slack', publisher: 'Slack Technologies' }),
  item({ id: 'ssh', name: 'ssh', kind: 'service', scope: 'system', canDelete: false }),
  item({ id: 'dbus', name: 'dbus', kind: 'service', scope: 'system', critical: true, canDisable: false, canDelete: false }),
  item({ id: 'job', name: 'backup', kind: 'cron', command: '@reboot /home/u/sync.sh' }),
  item({ id: 'ctx', name: '7-Zip', kind: 'context_menu' }),
];

describe('startup filters', () => {
  it('shows only kinds that are present, in a fixed order, counting what is visible', () => {
    expect(kindsPresent(ITEMS, false).map((k) => [k.kind, k.count])).toEqual([
      ['autostart', 1],
      ['service', 1],
      ['cron', 1],
      ['context_menu', 1],
    ]);
    expect(kindsPresent(ITEMS, true).find((k) => k.kind === 'service')?.count).toBe(2);
    expect(kindsPresent([], true)).toEqual([]);
  });

  it('hides critical items until asked', () => {
    const names = (v: StartupItem[]) => v.map((i) => i.id);
    expect(names(visibleItems(ITEMS, { query: '', kind: 'all', showSystem: false }))).toEqual(['slack', 'ssh', 'job', 'ctx']);
    expect(names(visibleItems(ITEMS, { query: '', kind: 'all', showSystem: true }))).toContain('dbus');
    expect(hiddenSystemCount(ITEMS)).toBe(1);
  });

  it('filters by kind and by search over name, publisher and command', () => {
    const f = (query: string, kind: 'all' | 'cron' | 'service' = 'all') =>
      visibleItems(ITEMS, { query, kind, showSystem: true }).map((i) => i.id);
    expect(f('', 'cron')).toEqual(['job']);
    expect(f('technologies')).toEqual(['slack']);
    expect(f('sync.sh')).toEqual(['job']);
    expect(f('  SLACK ')).toEqual(['slack']);
    expect(f('zzz')).toEqual([]);
    expect(f('d', 'service')).toEqual(['dbus']);
  });

  it('describes deletion and the row sub-line', () => {
    expect(deleteMessage(ITEMS[3]!)).toContain('every other line stays');
    expect(deleteMessage(ITEMS[0]!)).toContain('A backup is saved first');
    expect(subline(ITEMS[0]!)).toBe('Slack Technologies');
    expect(subline(ITEMS[1]!)).toBe('All users');
    expect(subline(ITEMS[3]!)).toBe('@reboot /home/u/sync.sh');
  });
});
