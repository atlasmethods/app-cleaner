import { describe, expect, it } from 'vitest';
import type { RestorePoint } from '../api/restore';
import {
  backupKindLabel,
  deleteBlockedReason,
  descriptionProblem,
  hasOlderWindowsPoints,
  kindLabel,
  restorablePoints,
  restoreMessage,
  splitPoints,
} from './restore';

const pt = (over: Partial<RestorePoint>): RestorePoint => ({
  id: 'x',
  description: 'd',
  createdAt: null,
  kind: 'timeshift',
  deletable: true,
  restorable: false,
  isNewest: false,
  ...over,
});

describe('delete rules', () => {
  it('explains why a point cannot be deleted', () => {
    expect(deleteBlockedReason(pt({}))).toBeNull();
    expect(deleteBlockedReason(pt({ deletable: false, isNewest: true }))).toContain('most recent');
    expect(deleteBlockedReason(pt({ deletable: false, kind: 'windows-restore-point' }))).toContain('in bulk');
    expect(deleteBlockedReason(pt({ deletable: false }))).toContain('cannot be deleted');
  });

  it('bulk deletion is offered only with several Windows points', () => {
    const w = pt({ kind: 'windows-restore-point' });
    expect(hasOlderWindowsPoints([w])).toBe(false);
    expect(hasOlderWindowsPoints([w, { ...w, id: 'y' }])).toBe(true);
    expect(hasOlderWindowsPoints([pt({}), pt({ id: 'z' })])).toBe(false);
  });
});

describe('restore confirmation copy', () => {
  it('is specific to what is put back', () => {
    expect(restoreMessage(pt({ backupKind: 'startup' }))).toContain('already exists');
    expect(restoreMessage(pt({ backupKind: 'plugins' }))).toContain('browser must be closed');
    expect(restoreMessage(pt({ backupKind: 'config' }))).toContain('overwritten');
  });
});

describe('labels and grouping', () => {
  it('kind labels', () => {
    expect(kindLabel('tmutil')).toBe('Time Machine');
    expect(kindLabel('windows-restore-point')).toBe('Windows');
    expect(backupKindLabel(pt({ backupKind: 'registry' }))).toBe('Registry backup');
    expect(backupKindLabel(pt({ backupKind: 'drivers' }))).toBe('Driver backup');
    expect(backupKindLabel(pt({ backupKind: 'startup' }))).toBe('Startup item');
    expect(backupKindLabel(pt({ backupKind: 'plugins' }))).toBe('Browser add-on');
    expect(backupKindLabel(pt({}))).toBe('Backup');
  });

  it('splits system points from backups and finds the restorable ones', () => {
    const a = pt({ id: 'a' });
    const b = pt({ id: 'b', kind: 'clearsweep-backup', restorable: true });
    const c = pt({ id: 'c', kind: 'clearsweep-backup' });
    const s = splitPoints([a, b, c]);
    expect(s.system.map((p) => p.id)).toEqual(['a']);
    expect(s.backups.map((p) => p.id)).toEqual(['b', 'c']);
    expect(restorablePoints([a, b, c]).map((p) => p.id)).toEqual(['b']);
  });
});

describe('description check mirrors the server', () => {
  it('accepts and rejects', () => {
    expect(descriptionProblem('Before update')).toBeNull();
    expect(descriptionProblem('   ')).not.toBeNull();
    expect(descriptionProblem('-rf')).toContain('dash');
    expect(descriptionProblem('a\nb')).toContain('control');
    expect(descriptionProblem('x'.repeat(201))).toContain('200');
    expect(descriptionProblem('x'.repeat(200))).toBeNull();
  });
});
