import { readFileSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
// @ts-expect-error plain ES module script (also run by scripts/check.sh)
import { checkContrast, contrast, over, parseHex } from '../../scripts/check-contrast.mjs';

describe('design token contrast', () => {
  it('computes WCAG ratios', () => {
    expect(contrast(parseHex('#000'), parseHex('#fff'))).toBeCloseTo(21, 1);
    expect(contrast(parseHex('#777777'), parseHex('#ffffff'))).toBeCloseTo(4.48, 1);
    expect(over([0, 0, 0], 0.5, [255, 255, 255])).toEqual([128, 128, 128]);
  });

  it('every text and non-text pair in index.css passes in both themes', () => {
    const css = readFileSync(path.resolve(import.meta.dirname, '../index.css'), 'utf8');
    const { rows, driftKeys } = checkContrast(css) as { rows: { ok: boolean; theme: string; fg: string; bg: string; ratio: number }[]; driftKeys: string[] };
    expect(rows.length).toBeGreaterThan(40);
    expect(rows.filter((r) => !r.ok).map((r) => `${r.theme}: ${r.fg} on ${r.bg} = ${r.ratio.toFixed(2)}`)).toEqual([]);
    expect(driftKeys).toEqual([]);
  });
});
