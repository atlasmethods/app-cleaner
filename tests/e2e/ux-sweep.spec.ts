import { auditPage, auditProblems, contrastProblems } from './helpers/audit';
import { expect, test } from './fixtures';
import { ALL_ROUTES } from './helpers/routes';

const ROUTES = [...ALL_ROUTES, '/no/such/page'];

// Every route, light and dark, at the project's width (320 and 380): nothing is clipped or
// scrolls sideways, tap targets are at least 40px, every control has a name, and the page
// logs no console errors (the `consoleErrors` fixture fails the test otherwise).
for (const scheme of ['light', 'dark'] as const) {
  test.describe(`sweep ${scheme}`, () => {
    test.use({ colorScheme: scheme });
    for (const route of ROUTES) {
      test(`${route}`, async ({ app }) => {
        await app.evaluate((h) => {
          location.hash = h;
        }, `#${route}`);
        await expect(app.getByTestId('appbar-title')).toBeVisible();
        await app.waitForLoadState('networkidle');
        const problems = auditProblems(await auditPage(app));
        expect(problems).toEqual([]);
        expect(await contrastProblems(app), 'rendered text contrast').toEqual([]);
        // Keyboard focus is visible: tabbing to the first control draws the focus ring.
        await app.keyboard.press('Tab');
        const ring = await app.evaluate(() => {
          const el = document.activeElement;
          if (!el || el === document.body) return 'none-focused';
          const cs = getComputedStyle(el);
          return cs.outlineStyle !== 'none' && parseFloat(cs.outlineWidth) >= 2 ? 'ring' : `no ring on ${el.tagName}`;
        });
        expect(['ring', 'none-focused']).toContain(ring);
      });
    }
  });
}

test('the bottom tab bar never moves or resizes between pages', async ({ app }) => {
  const box = async () => (await app.getByTestId('tabbar').boundingBox())!;
  const first = await box();
  const vh = app.viewportSize()!.height;
  expect(first.y + first.height).toBeCloseTo(vh, 0); // flush with the bottom edge
  for (const route of ALL_ROUTES) {
    await app.evaluate((h) => (location.hash = h), `#${route}`);
    await expect(app.getByTestId('appbar-title')).toBeVisible();
    const b = await box();
    expect([b.x, b.y, b.width, b.height], route).toEqual([first.x, first.y, first.width, first.height]);
  }
  // also while a long page scrolls
  await app.evaluate(() => (location.hash = '#/clean'));
  await app.locator('[data-scroll-root]').evaluate((el) => (el.scrollTop = 10_000));
  expect(await box()).toEqual(first);
});
