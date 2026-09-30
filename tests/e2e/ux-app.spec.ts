/** Wide two-pane layout, compact mode, theme, and the File Shredder. */
import { existsSync, mkdirSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { callApi, expect, test } from './fixtures';
import { auditPage, auditProblems } from './helpers/audit';
import { ALL_ROUTES, TOOL_TILES } from './helpers/routes';

test.skip(process.platform !== 'linux', 'uses the Linux layout of the fake machine');

// ---------------------------------------------------------------- wide layout

test.describe('wide layout (1280x800)', () => {
  test.use({ viewport: { width: 1280, height: 800 } });
  test.beforeEach(({}, info) => {
    test.skip(info.project.name !== 'w380', 'viewport is fixed here; run once');
  });

  test('navigation rail instead of the tab bar, content column at most 720px', async ({ app }) => {
    await expect(app.getByTestId('navrail')).toBeVisible();
    await expect(app.getByTestId('tabbar')).toHaveCount(0);
    await expect(app.locator('[data-layout="wide"]')).toHaveCount(1);
    // the five tabs and every tool are reachable from the rail
    for (const id of ['home', 'clean', 'tools', 'performance', 'settings']) {
      await expect(app.getByTestId('navrail').getByTestId(`tab-${id}`)).toBeVisible();
    }
    await expect(app.getByTestId('navrail-tools').locator('a')).toHaveCount(TOOL_TILES.length);
    const widths = await app.getByTestId('page-home').evaluate((el) => el.getBoundingClientRect().width);
    expect(widths).toBeLessThanOrEqual(720);
    expect(widths).toBeGreaterThan(400);
    // centred between the rail and the right edge
    const rail = (await app.getByTestId('navrail').boundingBox())!;
    const page = (await app.getByTestId('page-home').boundingBox())!;
    expect(page.x).toBeGreaterThan(rail.x + rail.width);
    await app.getByTestId('rail-wiper').click();
    await expect(app.getByTestId('page-wiper')).toBeVisible();
    await expect(app.getByTestId('rail-wiper')).toHaveAttribute('aria-current', 'page');
    await expect(app.getByTestId('navrail').getByTestId('tab-tools')).toHaveAttribute('aria-current', 'page'); // its parent tab
    await app.getByTestId('appbar-back').click();
    await expect(app.getByTestId('page-tools')).toBeVisible();
  });

  test('sticky action bars stay inside the content column', async ({ app }) => {
    await app.evaluate(() => (location.hash = '#/clean'));
    const bar = app.getByTestId('action-bar');
    await expect(bar).toBeVisible();
    const b = (await bar.boundingBox())!;
    expect(b.x + b.width).toBeLessThanOrEqual(1280);
    expect(b.width).toBeLessThanOrEqual(720);
  });

  test('breakpoint: 899px is compact, 900px is wide', async ({ app }) => {
    await app.setViewportSize({ width: 899, height: 700 });
    await expect(app.getByTestId('tabbar')).toBeVisible();
    await expect(app.getByTestId('navrail')).toHaveCount(0);
    await app.setViewportSize({ width: 900, height: 700 });
    await expect(app.getByTestId('navrail')).toBeVisible();
    await expect(app.getByTestId('tabbar')).toHaveCount(0);
  });

  test('compact mode forces the compact layout, survives reloads, and can be undone', async ({ app }) => {
    await app.getByTestId('tab-settings').click();
    const sw = app.getByTestId('setting-compact-mode');
    await expect(sw).toHaveAttribute('aria-checked', 'false');
    await sw.click();
    await expect(app.getByTestId('tabbar')).toBeVisible();
    await expect(app.getByTestId('navrail')).toHaveCount(0);
    const w = await app.locator('[data-layout="compact"]').evaluate((el) => el.getBoundingClientRect().width);
    expect(w).toBeLessThanOrEqual(480);
    // persisted on the server (survives a browser that forgot localStorage)
    await expect.poll(async () => (await callApi<{ compactMode: boolean }>(appServer(app), 'settings.get')).compactMode).toBe(true);
    await app.evaluate(() => window.localStorage.clear());
    await app.reload();
    await expect(app.getByTestId('tabbar')).toBeVisible();
    await app.getByTestId('setting-compact-mode').click();
    await expect(app.getByTestId('navrail')).toBeVisible();
    await expect(app.getByTestId('tabbar')).toHaveCount(0);
  });

  for (const scheme of ['light', 'dark'] as const) {
    test(`every route is clean at 1280 (${scheme})`, async ({ app }) => {
      await app.emulateMedia({ colorScheme: scheme });
      for (const route of ALL_ROUTES) {
        await app.evaluate((h) => (location.hash = h), `#${route}`);
        await expect(app.getByTestId('appbar-title')).toBeVisible();
        await app.waitForLoadState('networkidle');
        expect(auditProblems(await auditPage(app)), route).toEqual([]);
      }
    });
  }
});

// Helper: the server info is not passed around; read it back from the page's own origin + token.
function appServer(page: import('@playwright/test').Page) {
  const origin = new URL(page.url()).origin;
  return { origin, token: currentToken, url: origin, dir: '' };
}
let currentToken = '';
test.beforeEach(async ({ server }) => {
  currentToken = server.token;
});

// ---------------------------------------------------------------- theme

test.describe('theme', () => {
  test.beforeEach(({}, info) => {
    test.skip(info.project.name !== 'w380', 'theme is width independent');
  });
  const bg = (page: import('@playwright/test').Page) =>
    page.evaluate(() => getComputedStyle(document.body).backgroundColor);

  test('applies immediately, persists across reloads, and "system" follows the OS live', async ({ app }) => {
    await app.emulateMedia({ colorScheme: 'light' });
    await app.getByTestId('tab-settings').click();
    const light = await bg(app);
    await app.getByTestId('theme-dark').click();
    await expect(app.locator('html')).toHaveAttribute('data-theme', 'dark');
    const dark = await bg(app);
    expect(dark).not.toBe(light);
    // persisted in the settings file, so a fresh profile (no localStorage) gets it too
    await expect.poll(async () => (await callApi<{ theme: string }>(appServer(app), 'settings.get')).theme).toBe('dark');
    await app.evaluate(() => window.localStorage.clear());
    await app.reload();
    await expect(app.locator('html')).toHaveAttribute('data-theme', 'dark');
    expect(await bg(app)).toBe(dark);

    // light override beats an OS that prefers dark
    await app.emulateMedia({ colorScheme: 'dark' });
    await app.getByTestId('tab-settings').click();
    await app.getByTestId('theme-light').click();
    expect(await bg(app)).toBe(light);

    // system: no override, and it tracks prefers-color-scheme while the page is open
    await app.getByTestId('theme-system').click();
    await expect(app.locator('html')).not.toHaveAttribute('data-theme', /.+/);
    expect(await bg(app)).toBe(dark);
    await app.emulateMedia({ colorScheme: 'light' });
    await expect.poll(() => bg(app)).toBe(light);
    await app.emulateMedia({ colorScheme: 'dark' });
    await expect.poll(() => bg(app)).toBe(dark);
  });
});

// ---------------------------------------------------------------- File Shredder

test.describe('File Shredder', () => {
  test('shreds a file and a folder, refuses protected paths, reports each path', async ({ app, sandbox }) => {
    const file = path.join(sandbox.home, 'shred-me.txt');
    const dir = path.join(sandbox.home, 'shred-dir');
    mkdirSync(path.join(dir, 'sub'), { recursive: true });
    writeFileSync(file, Buffer.alloc(300_000, 0x41));
    writeFileSync(path.join(dir, 'a.bin'), 'secret');
    writeFileSync(path.join(dir, 'sub', 'b.bin'), 'secret too');
    const protectedPath = sandbox.home; // the home folder itself is never shredded

    await app.getByTestId('tab-tools').click();
    await app.getByTestId('tile-shredder').click();
    await expect(app.getByTestId('page-shredder')).toBeVisible();
    await expect(app.getByTestId('btn-shredder-start')).toBeDisabled();

    // a relative path is rejected before anything is sent
    await app.getByTestId('shredder-input').fill('relative/file.txt');
    await app.getByTestId('shredder-add').click();
    await expect(app.getByTestId('shredder-input-error')).toContainText('absolute');
    await expect(app.getByTestId('shredder-item')).toHaveCount(0);

    // several pasted at once
    await app.getByTestId('shredder-input').fill(`${file}\n"${dir}"\n${protectedPath}\n${file}`);
    await app.getByTestId('shredder-add').click();
    await expect(app.getByTestId('shredder-item')).toHaveCount(3);
    await expect(app.getByTestId('btn-shredder-start')).toHaveText('Shred 3 items');
    await app.getByTestId('shredder-passes').selectOption('3');

    await app.getByTestId('btn-shredder-start').click();
    const confirm = app.getByTestId('confirm-sheet-confirm');
    await expect(confirm).toBeDisabled();
    await app.getByTestId('confirm-sheet-input').fill('shred'); // case matters
    await expect(confirm).toBeDisabled();
    // nothing happened yet
    expect(existsSync(file)).toBe(true);
    await app.getByTestId('confirm-sheet-input').fill('SHRED');
    await expect(confirm).toBeEnabled();
    await confirm.click();

    await expect(app.getByTestId('shredder-result')).toBeVisible();
    await expect(app.getByTestId('shredder-result-ok')).toHaveCount(2);
    await expect(app.getByTestId('shredder-result-failed')).toHaveCount(1);
    await expect(app.getByTestId('shredder-result-failed')).toContainText(/protected/i);
    await expect(app.getByTestId('shredder-summary')).toContainText('Shredded 2 items');
    expect(existsSync(file)).toBe(false);
    expect(existsSync(dir)).toBe(false);
    expect(statSync(protectedPath).isDirectory()).toBe(true);
    // only the failed path stays in the list
    await expect(app.getByTestId('shredder-item')).toHaveCount(1);
    await expect(app.getByTestId('shredder-item')).toContainText(protectedPath);
  });

  test('the default passes come from Settings and cancelling the confirm shreds nothing', async ({ app, sandbox, server }) => {
    await callApi(server, 'settings.set', { secureDelete: { passes: 7 } });
    const file = path.join(sandbox.home, 'keep-me.txt');
    writeFileSync(file, 'keep');
    await app.evaluate(() => (location.hash = '#/tools/shredder'));
    await expect(app.getByTestId('shredder-passes')).toHaveValue('7');
    await app.getByTestId('shredder-input').fill(file);
    await app.getByTestId('shredder-input').press('Enter');
    await app.getByTestId('btn-shredder-start').click();
    await app.keyboard.press('Escape');
    await expect(app.getByRole('dialog')).toHaveCount(0);
    expect(existsSync(file)).toBe(true);
    // removing the entry from the list
    await app.getByRole('button', { name: `Remove ${file}` }).click();
    await expect(app.getByTestId('shredder-item')).toHaveCount(0);
    await callApi(server, 'settings.reset');
  });
});
