import type { Locator, Page } from '@playwright/test';

export interface AuditResult {
  /** Visible elements that extend past the viewport (clipped content or a horizontal scrollbar). */
  overflow: string[];
  /** Interactive elements smaller than 40x40 CSS px (checkbox/radio inputs count their label). */
  smallTargets: string[];
  /** Buttons / links / controls without an accessible name. */
  unnamed: string[];
  /** Text fields and selects without a bound label or aria-label. */
  unlabeled: string[];
  documentScrollWidth: number;
  viewportWidth: number;
}

/**
 * Runs in the page: structural accessibility / layout checks that must hold on every screen.
 * `scope` restricts the checks to a subtree (for example an open dialog).
 */
export async function auditPage(page: Page, scope?: string): Promise<AuditResult> {
  return page.evaluate((scopeSel) => {
    const root: ParentNode = scopeSel ? (document.querySelector(scopeSel) ?? document) : document;
    const vw = window.innerWidth;
    const desc = (el: Element): string => {
      const id = el.getAttribute('data-testid');
      const txt = (el.getAttribute('aria-label') ?? el.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 40);
      return `${el.tagName.toLowerCase()}${id ? `[${id}]` : ''}${txt ? ` "${txt}"` : ''}`;
    };
    const visible = (el: Element): boolean => {
      const r = el.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) return false;
      const cs = getComputedStyle(el);
      return cs.visibility !== 'hidden' && cs.display !== 'none';
    };
    const inHScroller = (el: Element): boolean => {
      for (let p: Element | null = el.parentElement; p; p = p.parentElement) {
        if (p.hasAttribute('data-allow-hscroll')) return true;
        const ox = getComputedStyle(p).overflowX;
        if ((ox === 'auto' || ox === 'scroll') && p.scrollWidth > p.clientWidth) return true;
      }
      return false;
    };

    const all = Array.from(root.querySelectorAll('*')).filter(visible);
    const overflow = all
      .filter((el) => {
        if (el.closest('.sr-only')) return false;
        const r = el.getBoundingClientRect();
        return (r.right > vw + 1 || r.left < -1) && !inHScroller(el);
      })
      .slice(0, 8)
      .map(desc);

    const targets = Array.from(
      root.querySelectorAll('a[href], button, [role="button"], [role="radio"], [role="switch"], [role="checkbox"], input:not([type="hidden"]), select, textarea'),
    ).filter(visible);
    const smallTargets: string[] = [];
    for (const el of targets) {
      if (el.closest('.sr-only')) continue;
      const big = (b: DOMRect) => b.width >= 39.5 && b.height >= 39.5;
      const r = el.getBoundingClientRect();
      let ok = big(r);
      if (!ok && el instanceof HTMLInputElement && (el.type === 'checkbox' || el.type === 'radio')) {
        // A label around (or bound to) the input is an equally good touch target.
        const label = el.closest('label') ?? el.labels?.[0];
        if (label) ok = big(label.getBoundingClientRect());
      }
      const isInline =
        el instanceof HTMLAnchorElement && getComputedStyle(el).display === 'inline' && !!el.closest('p, li, span');
      if (!ok && !isInline) {
        smallTargets.push(`${desc(el)} ${Math.round(r.width)}x${Math.round(r.height)}`);
      }
    }

    const nameOf = (el: Element): string => {
      const aria = el.getAttribute('aria-label')?.trim();
      if (aria) return aria;
      const by = el.getAttribute('aria-labelledby');
      if (by) {
        const t = by
          .split(/\s+/)
          .map((i) => document.getElementById(i)?.textContent ?? '')
          .join(' ')
          .trim();
        if (t) return t;
      }
      if (el instanceof HTMLInputElement || el instanceof HTMLSelectElement || el instanceof HTMLTextAreaElement) {
        const l = Array.from(el.labels ?? [])
          .map((x) => x.textContent ?? '')
          .join(' ')
          .trim();
        if (l) return l;
      }
      const text = (el.textContent ?? '').trim();
      if (text) return text;
      const img = el.querySelector('[aria-label]');
      if (img?.getAttribute('aria-label')) return img.getAttribute('aria-label')!;
      return el.getAttribute('title')?.trim() ?? '';
    };
    const unnamed = targets.filter((el) => !nameOf(el)).map(desc);
    const unlabeled = targets
      .filter((el) => el instanceof HTMLInputElement || el instanceof HTMLSelectElement || el instanceof HTMLTextAreaElement)
      .filter((el) => !nameOf(el))
      .map(desc);

    return {
      overflow,
      smallTargets,
      unnamed,
      unlabeled,
      documentScrollWidth: document.documentElement.scrollWidth,
      viewportWidth: vw,
    };
  }, scope ?? null);
}

/** Human readable failure list, empty when everything holds. */
export function auditProblems(a: AuditResult): string[] {
  const out: string[] = [];
  if (a.documentScrollWidth > a.viewportWidth) out.push(`document scrolls horizontally (${a.documentScrollWidth} > ${a.viewportWidth})`);
  for (const x of a.overflow) out.push(`overflows the viewport: ${x}`);
  for (const x of a.smallTargets) out.push(`tap target < 40px: ${x}`);
  for (const x of a.unnamed) out.push(`no accessible name: ${x}`);
  for (const x of a.unlabeled) out.push(`form control without a label: ${x}`);
  return out;
}

/**
 * Behaviour every open sheet must have: inside the viewport, scrollable (or scrolling its body)
 * when tall, focus moves in and is trapped, the page behind cannot scroll, Escape closes it and
 * focus returns to the control that opened it. `opener` is that control's locator.
 */
export async function checkSheet(page: Page, opener: Locator): Promise<void> {
  const { expect } = await import('@playwright/test');
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  const vp = page.viewportSize()!;
  const box = (await dialog.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(-0.5);
  expect(box.x + box.width).toBeLessThanOrEqual(vp.width + 0.5);
  expect(box.y).toBeGreaterThanOrEqual(-0.5);
  expect(box.y + box.height).toBeLessThanOrEqual(vp.height + 0.5);
  expect(auditProblems(await auditPage(page, '[role="dialog"]')).filter((p) => !p.startsWith('document scrolls'))).toEqual([]);

  expect(await contrastProblems(page, '[role="dialog"]'), 'sheet text contrast').toEqual([]);

  const inside = () => page.evaluate(() => !!document.activeElement?.closest('[role="dialog"]'));
  expect(await inside(), 'focus starts inside the dialog').toBe(true);
  for (let i = 0; i < 25; i++) {
    await page.keyboard.press('Tab');
    expect(await inside(), `focus stays inside after Tab #${i + 1}`).toBe(true);
  }
  await page.keyboard.press('Shift+Tab');
  expect(await inside(), 'focus stays inside after Shift+Tab').toBe(true);

  // Background: the page's scroller is locked and a wheel over the backdrop does nothing.
  const locked = await page.evaluate(() => getComputedStyle(document.querySelector('[data-scroll-root]')!).overflowY);
  expect(locked).toBe('hidden');
  const before = await page.evaluate(() => document.querySelector('[data-scroll-root]')!.scrollTop);
  await page.mouse.move(vp.width / 2, 4);
  await page.mouse.wheel(0, 400);
  expect(await page.evaluate(() => document.querySelector('[data-scroll-root]')!.scrollTop)).toBe(before);

  // A body taller than the sheet scrolls inside it.
  const scrolls = await dialog.evaluate((d) => {
    const el = [d, ...Array.from(d.querySelectorAll<HTMLElement>('*'))].find(
      (x) => ['auto', 'scroll'].includes(getComputedStyle(x).overflowY) && x.scrollHeight > x.clientHeight + 1,
    );
    if (!el) return 'fits';
    el.scrollTop = 10_000;
    return el.scrollTop > 0 ? 'scrolls' : 'stuck';
  });
  expect(['fits', 'scrolls']).toContain(scrolls);

  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await expect(opener).toBeFocused();
}

/**
 * Rendered text contrast (WCAG AA: 4.5:1, or 3:1 for large text), measured on the real page:
 * each element with its own text against the first opaque background behind it, translucent
 * fills composited. Disabled controls and anything faded with `opacity` are exempt.
 */
export async function contrastProblems(page: Page, scope?: string): Promise<string[]> {
  return page.evaluate((scopeSel) => {
    const root: ParentNode = scopeSel ? (document.querySelector(scopeSel) ?? document) : document;
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = 1;
    const cx = canvas.getContext('2d', { willReadFrequently: true })!;
    const parse = (css: string): [number, number, number, number] => {
      cx.clearRect(0, 0, 1, 1);
      cx.fillStyle = '#000';
      cx.fillStyle = css;
      cx.fillRect(0, 0, 1, 1);
      const d = cx.getImageData(0, 0, 1, 1).data;
      return [d[0]!, d[1]!, d[2]!, d[3]! / 255];
    };
    const over = (f: number[], b: number[]): [number, number, number, number] => {
      const a = f[3]!;
      return [0, 1, 2].map((i) => f[i]! * a + b[i]! * (1 - a)).concat([1]) as [number, number, number, number];
    };
    const lum = (c: number[]) => {
      const l = (v: number) => {
        const s = v / 255;
        return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
      };
      return 0.2126 * l(c[0]!) + 0.7152 * l(c[1]!) + 0.0722 * l(c[2]!);
    };
    const ratio = (a: number[], b: number[]) => {
      const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
      return (hi! + 0.05) / (lo! + 0.05);
    };
    const backdrop = (el: Element): [number, number, number, number] => {
      const layers: [number, number, number, number][] = [];
      for (let e: Element | null = el; e; e = e.parentElement) {
        const bg = parse(getComputedStyle(e).backgroundColor);
        if (bg[3] > 0) {
          layers.push(bg);
          if (bg[3] >= 1) break;
        }
      }
      let acc: [number, number, number, number] = parse(getComputedStyle(document.body).backgroundColor);
      if (acc[3] < 1) acc = [255, 255, 255, 1];
      for (const l of layers.reverse()) acc = over(l, acc);
      return acc;
    };
    const faded = (el: Element) => {
      for (let e: Element | null = el; e; e = e.parentElement) {
        if (parseFloat(getComputedStyle(e).opacity) < 1) return true;
        if ((e as HTMLElement).matches?.(':disabled, [aria-disabled="true"]')) return true;
      }
      return false;
    };
    const out: string[] = [];
    const seen = new Set<string>();
    for (const el of Array.from(root.querySelectorAll('*'))) {
      const own = Array.from(el.childNodes).some((n) => n.nodeType === 3 && (n.textContent ?? '').trim());
      if (!own || el.closest('.sr-only, svg, option')) continue;
      const r = el.getBoundingClientRect();
      const cs = getComputedStyle(el);
      if (r.width === 0 || r.height === 0 || cs.visibility === 'hidden' || faded(el)) continue;
      const fgRaw = parse(cs.color);
      const bg = backdrop(el);
      const fg = over(fgRaw, bg);
      const size = parseFloat(cs.fontSize);
      const bold = parseInt(cs.fontWeight, 10) >= 700;
      const large = size >= 24 || (size >= 18.66 && bold);
      const need = large ? 3 : 4.5;
      const got = ratio(fg, bg);
      if (got < need - 0.01) {
        const id = el.getAttribute('data-testid');
        const text = (el.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 30);
        const key = `${el.tagName}|${cs.color}|${bg.join(',')}`;
        if (seen.has(key)) continue;
        seen.add(key);
        out.push(`${el.tagName.toLowerCase()}${id ? `[${id}]` : ''} "${text}" ${got.toFixed(2)}:1 (need ${need}) fg ${cs.color} on rgb(${bg.slice(0, 3).map(Math.round).join(',')})`);
      }
    }
    return out.slice(0, 10);
  }, scope ?? null);
}
