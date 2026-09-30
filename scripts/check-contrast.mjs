#!/usr/bin/env node
// WCAG contrast check for the design tokens in src/index.css (both themes).
//   node scripts/check-contrast.mjs        -> prints a table, exits 1 when a pair fails
// Text pairs must reach 4.5:1; non-text pairs (focus ring, selected borders) 3:1.
// Translucent fills such as `bg-danger/10` are composited over the surface they sit on.
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

export function parseHex(hex) {
  const h = hex.trim().replace('#', '');
  const full = h.length === 3 ? [...h].map((c) => c + c).join('') : h;
  return [0, 2, 4].map((i) => parseInt(full.slice(i, i + 2), 16));
}

/** Composite `fg` (rgb) at `alpha` over `bg` (rgb). */
export function over(fg, alpha, bg) {
  return fg.map((c, i) => Math.round(c * alpha + bg[i] * (1 - alpha)));
}

export function luminance([r, g, b]) {
  const lin = (v) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

export function contrast(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** `{ light: {bg: '#..'}, dark: {...} }` from the token blocks of index.css. */
export function parseThemes(css) {
  const block = (selector) => {
    const i = css.indexOf(selector);
    if (i < 0) throw new Error(`index.css: no block for ${selector}`);
    const open = css.indexOf('{', i);
    const close = css.indexOf('}', open);
    const vars = {};
    for (const m of css.slice(open + 1, close).matchAll(/--([a-z0-9-]+):\s*(#[0-9a-fA-F]{3,6})\s*;/g)) vars[m[1]] = m[2];
    return vars;
  };
  return {
    light: block(':root {'),
    dark: block(":root[data-theme='dark'] {"),
    darkMedia: block(":root:not([data-theme='light']) {"),
  };
}

const T = 4.5; // text
const N = 3; // non-text

/** [foreground, background, minimum, description]; a background of `token/alpha@surface` is composited. */
export const PAIRS = [
  ['fg', 'bg', T, 'body text on page'],
  ['fg', 'surface', T, 'body text on card'],
  ['fg', 'surface-2', T, 'body text on inset controls'],
  ['muted', 'bg', T, 'secondary text on page'],
  ['muted', 'surface', T, 'secondary text on card / tab bar'],
  ['muted', 'surface-2', T, 'secondary text on inset controls'],
  ['accent', 'surface', T, 'accent text / links on card'],
  ['accent', 'bg', T, 'accent text on page'],
  ['accent-strong', 'surface', T, 'active tab label'],
  ['accent-strong', 'accent/0.1@surface', T, 'selected chip label'],
  ['accent-strong', 'accent/0.1@surface-2', T, 'selected chip label on inset'],
  ['accent-strong', 'accent/0.15@surface-2', T, 'accent badge'],
  ['accent-fg', 'accent', T, 'primary button label'],
  ['danger-fg', 'danger', T, 'destructive button label'],
  ['danger', 'surface', T, 'error / destructive text on card'],
  ['danger', 'bg', T, 'error text on page'],
  ['danger', 'danger/0.1@surface', T, 'error banner text'],
  ['danger', 'danger/0.1@bg', T, 'error banner text on page'],
  ['danger', 'danger/0.1@surface-2', T, 'danger badge on inset rows'],
  ['fg', 'danger/0.1@surface', T, 'banner body text'],
  ['warn', 'surface', T, 'warning text on card'],
  ['warn', 'bg', T, 'warning text on page'],
  ['warn', 'warn/0.1@surface', T, 'warning banner text'],
  ['warn', 'warn/0.1@bg', T, 'warning banner text on page'],
  ['ok', 'surface', T, 'success text on card'],
  ['ok', 'ok/0.1@surface', T, 'success banner text'],
  ['ok', 'ok/0.1@bg', T, 'success banner text on page'],
  ['fg', 'ok/0.1@surface', T, 'success body text'],
  ['accent', 'bg', N, 'focus ring on page'],
  ['accent', 'surface', N, 'focus ring on card'],
  ['accent', 'surface-2', N, 'focus ring on inset'],
  ['accent', 'surface', N, 'icons / progress fill on card'],
];

function resolve(vars, spec) {
  const m = /^([a-z0-9-]+)\/([0-9.]+)@([a-z0-9-]+)$/.exec(spec);
  if (!m) return parseHex(vars[spec]);
  return over(parseHex(vars[m[1]]), Number(m[2]), parseHex(vars[m[3]]));
}

export function checkContrast(css) {
  const themes = parseThemes(css);
  const rows = [];
  for (const theme of ['light', 'dark']) {
    const vars = themes[theme];
    for (const [fg, bg, min, what] of PAIRS) {
      for (const k of [fg, bg.split('/')[0].split('@')[0]]) {
        if (!vars[k]) throw new Error(`index.css: --${k} missing in the ${theme} theme`);
      }
      const ratio = contrast(resolve(vars, fg), resolve(vars, bg));
      rows.push({ theme, fg, bg, min, what, ratio, ok: ratio >= min });
    }
  }
  const driftKeys = Object.keys(themes.dark).filter((k) => themes.dark[k] !== themes.darkMedia[k]);
  return { rows, driftKeys };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const css = readFileSync(path.join(root, 'src/index.css'), 'utf8');
  const { rows, driftKeys } = checkContrast(css);
  let bad = 0;
  for (const r of rows) {
    if (!r.ok) bad++;
    console.log(
      `${r.ok ? 'ok  ' : 'FAIL'} ${r.theme.padEnd(5)} ${r.ratio.toFixed(2).padStart(5)} (min ${r.min})  ${r.fg} on ${r.bg}  - ${r.what}`,
    );
  }
  if (driftKeys.length) {
    bad++;
    console.log(`FAIL dark theme tokens differ between the media query and [data-theme=dark]: ${driftKeys.join(', ')}`);
  }
  console.log(bad ? `\n${bad} problem(s)` : '\nall contrast pairs pass');
  process.exit(bad ? 1 : 0);
}
