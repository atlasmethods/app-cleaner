import { describe, expect, it } from 'vitest';
import { formatBytes, formatDuration, percent } from './format';

describe('format', () => {
  it('formats bytes', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(5 * 1024 ** 3)).toBe('5.0 GB');
    expect(formatBytes(-1)).toBe('—');
  });
  it('formats durations', () => {
    expect(formatDuration(93784)).toBe('1d 2h 3m');
    expect(formatDuration(3700)).toBe('1h 1m');
    expect(formatDuration(75)).toBe('1m 15s');
  });
  it('computes bounded percent', () => {
    expect(percent(50, 200)).toBe(25);
    expect(percent(1, 0)).toBe(0);
    expect(percent(300, 200)).toBe(100);
  });
});
