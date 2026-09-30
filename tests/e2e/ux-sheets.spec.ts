import { checkSheet } from './helpers/audit';
import { expect, test } from './fixtures';

// Every kind of bottom / confirm sheet reachable on the default fake machine, at the project width
// (320 and 380) and once more in a very short window: inside the viewport, focus trapped, page
// behind locked, Escape closes and returns focus.

async function go(page: import('@playwright/test').Page, hash: string): Promise<void> {
  await page.evaluate((h) => {
    location.hash = h;
  }, hash);
}

test('Clean: confirm sheet', async ({ app }) => {
  await go(app, '#/clean');
  await app.getByTestId('btn-analyze').click();
  await expect(app.getByTestId('btn-clean')).toBeEnabled();
  const clean = app.getByTestId('btn-clean');
  await clean.click();
  await checkSheet(app, clean);
});

test('Schedules: add-schedule sheet', async ({ app }) => {
  await go(app, '#/settings/schedules');
  const add = app.getByTestId('schedule-add');
  await add.click();
  await checkSheet(app, add);
});

test('Duplicates: options sheet', async ({ app }) => {
  await go(app, '#/tools/duplicates');
  const opt = app.getByTestId('btn-dup-options');
  await opt.click();
  await checkSheet(app, opt);
});

test('Home: findings sheet after a scan', async ({ app }) => {
  await app.getByTestId('btn-scan').click();
  await expect(app.getByTestId('btn-scan')).toBeEnabled();
  const card = app.locator('[data-testid^="cat-"]:not([data-testid$="-status"]):not([data-testid$="-summary"])').first();
  await expect(card).toBeVisible();
  await card.click();
  await checkSheet(app, card);
});

test('Drive Wiper: confirm sheet', async ({ app }) => {
  await go(app, '#/tools/wiper');
  await app.locator('[data-testid^="wiper-drive-"]').first().click();
  const start = app.getByTestId('btn-wiper-start');
  await start.click();
  await checkSheet(app, start);
});

test('File Shredder: strong confirm sheet', async ({ app }) => {
  await go(app, '#/tools/shredder');
  await app.getByTestId('shredder-input').fill('/tmp/clearsweep-never-created/a-rather-long-file-name-that-has-to-wrap-inside-the-sheet-without-overflowing.txt');
  await app.getByTestId('shredder-add').click();
  const start = app.getByTestId('btn-shredder-start');
  await start.click();
  await expect(app.getByTestId('confirm-sheet-confirm')).toBeDisabled();
  await checkSheet(app, start);
});

test('Cookies: delete confirm sheet', async ({ app }) => {
  await go(app, '#/tools/cookies');
  await app.locator('[data-testid^="cookie-select-"]').first().check();
  const del = app.getByTestId('btn-cookies-delete');
  await del.click();
  await checkSheet(app, del);
});

test.describe('in a very short window', () => {
  test.use({ viewport: { width: 320, height: 300 } });
  test('Shredder confirm stays reachable', async ({ app }) => {
    await go(app, '#/tools/shredder');
    await app.getByTestId('shredder-input').fill('/tmp/clearsweep-never-created/x.txt');
    await app.getByTestId('shredder-add').click();
    await app.getByTestId('btn-shredder-start').click();
    await expect(app.getByTestId('confirm-sheet-confirm')).toBeInViewport();
    await expect(app.getByTestId('confirm-sheet-cancel')).toBeInViewport();
    await app.getByTestId('confirm-sheet-cancel').click();
    await expect(app.getByRole('dialog')).toHaveCount(0);
  });
  test('Schedule sheet scrolls', async ({ app }) => {
    await go(app, '#/settings/schedules');
    const add = app.getByTestId('schedule-add');
    await add.click();
    await checkSheet(app, add);
    const dlg = app.getByRole('dialog');
    await expect(dlg).toHaveCount(0);
    await add.click();
    const info = await app.getByRole('dialog').evaluate((d) => ({ sh: d.scrollHeight, ch: d.clientHeight, oy: getComputedStyle(d).overflowY }));
    expect(info.sh).toBeGreaterThan(info.ch); // taller than the window...
    expect(info.oy).toBe('auto'); // ...and scrolls
  });
});
