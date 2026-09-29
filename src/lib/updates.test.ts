import { describe, expect, it } from 'vitest';
import type { UpdateEntry } from '../api/software_updater';
import { driverSourceLabel, driverVersions } from './drivers';
import { isWindowsClient } from './platform';
import {
  allUpdatable,
  effectiveSelection,
  resultById,
  selectable,
  toggleId,
  updateSourceLabel,
  versionChange,
} from './updates';

const u = (id: string, ignored = false): UpdateEntry => ({
  id,
  name: id,
  currentVersion: '1',
  newVersion: '2',
  source: 'apt',
  ignored,
});

describe('updates', () => {
  const entries = [u('apt:a'), u('apt:b', true), u('apt:c')];

  it('ignored entries are not selectable', () => {
    expect(selectable(entries).map((e) => e.id)).toEqual(['apt:a', 'apt:c']);
    expect(allUpdatable(entries)).toBe(2);
  });

  it('effective selection drops ignored and vanished ids', () => {
    expect(effectiveSelection(entries, new Set(['apt:a', 'apt:b', 'apt:gone']))).toEqual(['apt:a']);
    expect(effectiveSelection(entries, new Set())).toEqual([]);
  });

  it('toggleId returns a new set', () => {
    const s = new Set(['a']);
    const on = toggleId(s, 'b');
    expect([...on].sort()).toEqual(['a', 'b']);
    expect(s.size).toBe(1);
    expect([...toggleId(on, 'a')]).toEqual(['b']);
  });

  it('version change text', () => {
    expect(versionChange({ currentVersion: '1.0', newVersion: '2.0' })).toBe('1.0 → 2.0');
    expect(versionChange({ currentVersion: '', newVersion: '2.0' })).toBe('→ 2.0');
    expect(versionChange({ currentVersion: '1', newVersion: '' })).toBe('1 → newer');
  });

  it('result lookup and labels', () => {
    const m = resultById([{ id: 'x', name: 'X', ok: true, message: '' }]);
    expect(m.get('x')?.ok).toBe(true);
    expect(updateSourceLabel('brew-cask')).toBe('Cask');
    expect(updateSourceLabel('winget')).toBe('winget');
  });
});

describe('drivers', () => {
  it('formats versions', () => {
    expect(driverVersions({ currentVersion: '1.0', newVersion: '1.1' })).toBe('1.0 → 1.1');
    expect(driverVersions({ newVersion: '22.1' })).toBe('→ 22.1');
    expect(driverVersions({ currentVersion: '1.0' })).toBe('1.0');
    expect(driverVersions({})).toBe('');
    expect(driverSourceLabel('fwupd')).toBe('Firmware');
  });
});

describe('platform', () => {
  it('detects Windows from the user agent', () => {
    expect(isWindowsClient({ userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/124' })).toBe(true);
    expect(isWindowsClient({ userAgent: 'Mozilla/5.0 (X11; Linux x86_64) Chrome/124' })).toBe(false);
    expect(isWindowsClient({ userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)' })).toBe(false);
    expect(isWindowsClient(undefined)).toBe(false);
  });
});
