import { describe, expect, it } from 'vitest';
import type { WiperDevice } from '../api/wiper';
import { DEFAULT_OPTIONS, matchSummary, toScanParams, type DupOptions } from './dupOptions';
import { PASS_OPTIONS, canWipeDrive, confirmMatches, deviceBlockReason, isPasses } from './wipe';

const dev = (over: Partial<WiperDevice> = {}): WiperDevice => ({
  device: '/dev/sdb',
  name: 'sdb',
  sizeBytes: 1 << 30,
  removable: true,
  model: 'USB',
  partitions: [],
  mounts: [],
  swap: false,
  holders: [],
  isSystem: false,
  ...over,
});

describe('wipe rules', () => {
  it('offers exactly the four supported pass counts', () => {
    expect(PASS_OPTIONS.map((p) => p.value)).toEqual([1, 3, 7, 35]);
    expect([1, 3, 7, 35].every(isPasses)).toBe(true);
    expect([0, 2, 5, 34].some(isPasses)).toBe(false);
  });

  it('the typed confirmation must equal the device path exactly', () => {
    expect(confirmMatches('/dev/sdb', '/dev/sdb')).toBe(true);
    for (const typed of ['', 'sdb', '/dev/sdb ', ' /dev/sdb', '/dev/SDB', '/dev/sdb1']) {
      expect(confirmMatches('/dev/sdb', typed), typed).toBe(false);
    }
    expect(confirmMatches('', '')).toBe(false);
  });

  it('system drives and drives in use are never wipeable', () => {
    expect(deviceBlockReason(dev())).toBeNull();
    expect(deviceBlockReason(dev({ isSystem: true }))).toMatch(/System drive/);
    expect(deviceBlockReason(dev({ mounts: ['/mnt/usb'] }))).toMatch(/mounted at \/mnt\/usb/);
    expect(deviceBlockReason(dev({ swap: true }))).toMatch(/swap/);
    expect(deviceBlockReason(dev({ holders: ['dm-0'] }))).toMatch(/dm-0/);
  });

  it('canWipeDrive needs a free device and the typed path', () => {
    expect(canWipeDrive(null, '/dev/sdb')).toBe(false);
    expect(canWipeDrive(dev(), '')).toBe(false);
    expect(canWipeDrive(dev(), '/dev/sdb')).toBe(true);
    expect(canWipeDrive(dev({ isSystem: true }), '/dev/sdb')).toBe(false);
    expect(canWipeDrive(dev({ mounts: ['/'] }), '/dev/sdb')).toBe(false);
  });
});

describe('duplicate finder options', () => {
  const opts = (over: Partial<DupOptions>): DupOptions => ({ ...DEFAULT_OPTIONS, paths: ['/data'], ...over });

  it('builds parameters with megabyte filters converted to bytes', () => {
    const r = toScanParams(opts({ minMb: '1.5', maxMb: '10', hidden: true }));
    expect('params' in r && r.params).toMatchObject({
      paths: ['/data'],
      minSize: 1572864,
      maxSize: 10485760,
      includeHidden: true,
      includeSystem: false,
      skipZeroByte: true,
      followLinks: false,
      matchBy: { content: true, name: false, size: false, modified: false },
    });
  });

  it('rejects missing folders, no criteria and bad numbers', () => {
    expect(toScanParams(DEFAULT_OPTIONS)).toEqual({ error: 'Add at least one folder to search.' });
    const none = { name: false, size: false, modified: false, content: false };
    expect(toScanParams(opts({ match: none }))).toHaveProperty('error');
    expect(toScanParams(opts({ minMb: 'abc' }))).toHaveProperty('error');
    expect(toScanParams(opts({ minMb: '-1' }))).toHaveProperty('error');
    expect(toScanParams(opts({ minMb: '5', maxMb: '1' }))).toHaveProperty('error');
  });

  it('omits empty limits and exclusions', () => {
    const r = toScanParams(opts({}));
    expect('params' in r && r.params.minSize).toBeUndefined();
    expect('params' in r && r.params.excludePaths).toBeUndefined();
  });

  it('summarises the criteria', () => {
    expect(matchSummary(DEFAULT_OPTIONS.match)).toBe('content');
    expect(matchSummary({ name: true, size: true, modified: true, content: false })).toBe('name + size + date');
    expect(matchSummary({ name: false, size: false, modified: false, content: false })).toBe('nothing selected');
  });
});
