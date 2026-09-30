import type { Page } from '@playwright/test';

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
      let r = el.getBoundingClientRect();
      if (el instanceof HTMLInputElement && (el.type === 'checkbox' || el.type === 'radio')) {
        const label = el.closest('label') ?? el.labels?.[0];
        if (label) {
          const lr = label.getBoundingClientRect();
          if (lr.width * lr.height > r.width * r.height) r = lr;
        }
      }
      const isInline =
        el instanceof HTMLAnchorElement && getComputedStyle(el).display === 'inline' && !!el.closest('p, li, span');
      if ((r.height < 39.5 || r.width < 39.5) && !isInline) {
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
