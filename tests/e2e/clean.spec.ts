import { existsSync, mkdirSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { callApi, expect, noHorizontalScroll, test } from './fixtures';

test.skip(process.platform !== 'linux', 'the fake machine uses the Linux layout');

interface CookieDomain {
  domain: string;
  count: number;
  browsers: string[];
  kept: boolean;
}

test.describe('cleaning', () => {
  test('Analyze shows the Chrome cache size and Clean removes exactly those files, honouring exclusions', async ({
    app,
    server,
    sandbox,
  }) => {
    await callApi(server, 'settings.set', { selectedRules: ['chrome.cache', 'linux.temp'] });
    const data0 = path.join(sandbox.chromeCache(), 'Cache/Cache_Data/data_0');
    const keepme = path.join(sandbox.chromeCache(), 'Cache/Cache_Data/keepme.bin');
    const keepme2 = path.join(sandbox.chromeCache('Profile 1'), 'Cache/Cache_Data/keepme.bin');
    const bookmarks = path.join(sandbox.chromeData(), 'Bookmarks');
    for (const f of [data0, keepme, bookmarks]) expect(existsSync(f), f).toBe(true);

    // Exclude one file through the Settings UI.
    await app.getByTestId('tab-settings').click();
    await expect(app.getByTestId('page-settings')).toBeVisible();
    await app.getByTestId('exclude-pattern').fill(keepme);
    await app.getByTestId('exclude-add').click();
    await expect(app.getByTestId('exclude-list')).toContainText('keepme.bin');
    let m = await noHorizontalScroll(app);
    expect(m.scrollWidth).toBeLessThanOrEqual(m.innerWidth);

    await app.getByTestId('tab-clean').click();
    await expect(app.getByTestId('page-clean')).toBeVisible();
    await expect(app.getByTestId('btn-clean')).toBeDisabled();
    await app.getByTestId('btn-analyze').click();

    // Two Chrome profiles: 100 KiB + (100 KiB + 4 KiB keepme.bin of "Profile 1") = 204 KiB.
    await expect(app.getByTestId('result-chrome.cache')).toBeVisible();
    await expect(app.getByTestId('result-chrome.cache-size')).toHaveText('204 KB');
    await expect(app.getByTestId('result-chrome.cache')).toContainText('9 files');
    await expect(app.getByTestId('result-linux.temp')).toBeVisible();
    // tapping a result reveals the files that would go
    await app.getByTestId('result-chrome.cache-toggle').click();
    await expect(app.getByTestId('result-chrome.cache-details')).toContainText('data_0');
    await expect(app.getByTestId('result-chrome.cache-details')).not.toContainText(path.join(sandbox.chromeCache(), 'Cache/Cache_Data/keepme.bin'));
    m = await noHorizontalScroll(app);
    expect(m.scrollWidth).toBeLessThanOrEqual(m.innerWidth);
    // analysis is read-only
    expect(existsSync(data0)).toBe(true);

    await app.getByTestId('btn-clean').click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('This will permanently delete');
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('clean-summary')).toBeVisible();
    await expect(app.getByTestId('summary-chrome.cache')).toBeVisible();

    // on disk
    expect(existsSync(data0)).toBe(false);
    expect(existsSync(path.join(sandbox.chromeCache(), 'Code Cache/js/index'))).toBe(false);
    expect(existsSync(path.join(sandbox.chromeCache('Profile 1'), 'Cache/Cache_Data/data_0'))).toBe(false);
    expect(existsSync(keepme), 'excluded file survives').toBe(true);
    expect(existsSync(keepme2), 'not excluded: removed').toBe(false);
    expect(existsSync(bookmarks)).toBe(true);
    expect(existsSync(path.join(sandbox.chromeData(), 'Network/Cookies'))).toBe(true);
    // other browsers were not selected
    expect(existsSync(path.join(sandbox.home, '.cache/microsoft-edge/Default/Cache/Cache_Data/data_0'))).toBe(true);
    // old temp file gone, recent one kept
    const tmp = path.join(sandbox.dir, 'root', 'tmp');
    expect(existsSync(path.join(tmp, 'old-1.tmp'))).toBe(false);
    expect(existsSync(path.join(tmp, 'new.tmp'))).toBe(true);
    m = await noHorizontalScroll(app);
    expect(m.scrollWidth).toBeLessThanOrEqual(m.innerWidth);

    // the run shows up in the history
    await app.getByTestId('history-link').click();
    await expect(app.getByTestId('page-clean-history')).toBeVisible();
    await expect(app.getByTestId('appbar-title')).toHaveText('History');
    await expect(app.locator('[data-testid^="history-"]').first()).toContainText('Manual');
    m = await noHorizontalScroll(app);
    expect(m.scrollWidth).toBeLessThanOrEqual(m.innerWidth);
    await app.getByTestId('appbar-back').click();
    await expect(app.getByTestId('page-clean')).toBeVisible();
  });

  test('rule selection is grouped, warns before dangerous rules, and persists across reloads', async ({ app, server }) => {
    await app.getByTestId('tab-clean').click();
    await expect(app.getByTestId('category-browser')).toBeVisible();
    await expect(app.getByTestId('category-system')).toBeVisible();
    await expect(app.getByTestId('category-application')).toBeVisible();
    await app.getByTestId('group-expand-google-chrome').click();
    await expect(app.getByTestId('rule-chrome.cache')).toBeChecked();
    const pw = app.getByTestId('rule-chrome.passwords');
    await expect(pw).not.toBeChecked();
    const group = app.getByTestId('group-check-google-chrome');
    expect(await group.evaluate((el) => (el as HTMLInputElement).indeterminate)).toBe(true);

    // enabling passwords needs an explicit confirmation
    await pw.click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('permanently deletes every password');
    await app.getByTestId('confirm-sheet').getByRole('button', { name: 'Cancel' }).click();
    await expect(pw).not.toBeChecked();

    // plain toggle persists
    await app.getByTestId('rule-chrome.history').uncheck();
    await expect
      .poll(async () => (await callApi<{ selectedRules: string[] }>(server, 'settings.get')).selectedRules ?? [])
      .not.toContain('chrome.history');
    await app.reload();
    await app.getByTestId('tab-clean').click();
    await app.getByTestId('group-expand-google-chrome').click();
    await expect(app.getByTestId('rule-chrome.history')).not.toBeChecked();
    await expect(app.getByTestId('rule-chrome.cache')).toBeChecked();
    const m = await noHorizontalScroll(app);
    expect(m.scrollWidth).toBeLessThanOrEqual(m.innerWidth);
  });
});

test.describe('cookies', () => {
  test('a kept domain survives cleaning while other cookies are removed', async ({ app, server, sandbox }) => {
    await callApi(server, 'settings.set', { selectedRules: ['chrome.cookies'] });
    await app.getByTestId('tab-tools').click();
    await app.getByTestId('tile-cookies').click();
    await expect(app.getByTestId('page-cookies')).toBeVisible();
    await expect(app.getByTestId('cookies-all')).toContainText('Cookies on this computer');
    await expect(app.getByTestId('cookies-keep')).toContainText('Cookies to keep');
    await expect(app.getByTestId('cookie-tracker.example')).toBeVisible();

    // keep google.com (moves it, and accounts.google.com under it)
    await app.getByTestId('cookie-keep-google.com').click();
    await expect(app.getByTestId('keep-google.com')).toBeVisible();
    await expect(app.getByTestId('cookie-accounts.google.com')).toHaveCount(0);
    await expect(app.getByTestId('cookie-notgoogle.com')).toBeVisible();
    await expect.poll(async () => (await callApi<{ domains: string[] }>(server, 'cookies.get_keep_list')).domains).toEqual(['google.com']);
    let m = await noHorizontalScroll(app);
    expect(m.scrollWidth).toBeLessThanOrEqual(m.innerWidth);

    // search
    await app.getByTestId('cookies-search').fill('tracker');
    await expect(app.getByTestId('cookie-tracker.example')).toBeVisible();
    await expect(app.getByTestId('cookie-notgoogle.com')).toHaveCount(0);
    await app.getByTestId('cookies-search').fill('');

    await app.getByTestId('tab-clean').click();
    await app.getByTestId('btn-analyze').click();
    await expect(app.getByTestId('result-chrome.cookies')).toBeVisible();
    // Per Chrome profile (x2): tracker.example x2, github x1, notgoogle x1 = 4 rows -> 8
    await expect(app.getByTestId('result-chrome.cookies')).toContainText('8 entries');
    await app.getByTestId('btn-clean').click();
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('clean-summary')).toBeVisible();

    const list = await callApi<CookieDomain[]>(server, 'cookies.list');
    const chromeDomains = list.filter((d) => d.browsers.includes('Google Chrome')).map((d) => d.domain).sort();
    expect(chromeDomains).toEqual(['accounts.google.com', 'google.com']);
    // lookalike and tracker were deleted from Chrome, but other browsers were not selected
    expect(list.find((d) => d.domain === 'notgoogle.com')?.browsers).not.toContain('Google Chrome');
    expect(list.find((d) => d.domain === 'tracker.example')?.browsers).toContain('Microsoft Edge');
    expect(existsSync(path.join(sandbox.chromeData(), 'Network/Cookies'))).toBe(true);
    expect(readdirSync(sandbox.chromeData()).includes('Bookmarks')).toBe(true);
    m = await noHorizontalScroll(app);
    expect(m.scrollWidth).toBeLessThanOrEqual(m.innerWidth);
  });

  test('Smart keep adds well-known sites and multi-select delete removes chosen sites', async ({ app, server }) => {
    await app.getByTestId('tab-tools').click();
    await app.getByTestId('tile-cookies').click();
    await app.getByTestId('btn-smart-keep').click();
    await expect(app.getByTestId('cookies-note')).toContainText('well-known');
    await expect(app.getByTestId('keep-google.com')).toBeVisible();
    await expect(app.getByTestId('keep-github.com')).toBeVisible();
    const keep = await callApi<{ domains: string[] }>(server, 'cookies.get_keep_list');
    expect(keep.domains).toEqual(expect.arrayContaining(['google.com', 'github.com']));
    expect(keep.domains).not.toContain('notgoogle.com');

    await app.getByTestId('cookie-select-tracker.example').check();
    await app.getByTestId('btn-cookies-delete').click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('Delete cookies of 1 site?');
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('cookies-note')).toContainText('Deleted');
    await expect(app.getByTestId('cookie-tracker.example')).toHaveCount(0);
    const list = await callApi<CookieDomain[]>(server, 'cookies.list');
    expect(list.some((d) => d.domain === 'tracker.example')).toBe(false);
    expect(list.some((d) => d.domain === 'google.com')).toBe(true);
    const m = await noHorizontalScroll(app);
    expect(m.scrollWidth).toBeLessThanOrEqual(m.innerWidth);
  });
});

test.describe('settings', () => {
  test('the theme toggle applies immediately and persists after reload', async ({ app, server }) => {
    await app.getByTestId('tab-settings').click();
    await expect(app.getByTestId('page-settings')).toBeVisible();
    await app.getByTestId('theme-dark').click();
    await expect(app.locator('html')).toHaveAttribute('data-theme', 'dark');
    await expect.poll(async () => (await callApi<{ theme: string }>(server, 'settings.get')).theme).toBe('dark');

    await app.reload();
    await expect(app.locator('html')).toHaveAttribute('data-theme', 'dark');
    await app.getByTestId('tab-settings').click();
    await expect(app.getByTestId('theme-dark')).toHaveAttribute('aria-checked', 'true');

    // even with the browser-side copy gone, the server-side setting wins
    await app.evaluate(() => window.localStorage.clear());
    await app.reload();
    await expect(app.locator('html')).toHaveAttribute('data-theme', 'dark');

    await app.getByTestId('tab-settings').click();
    await app.getByTestId('theme-light').click();
    await expect(app.locator('html')).toHaveAttribute('data-theme', 'light');
    await app.reload();
    await expect(app.locator('html')).toHaveAttribute('data-theme', 'light');
    await app.getByTestId('tab-settings').click();
    await app.getByTestId('theme-system').click();
    await expect(app.locator('html')).not.toHaveAttribute('data-theme', /.+/);
    const m = await noHorizontalScroll(app);
    expect(m.scrollWidth).toBeLessThanOrEqual(m.innerWidth);
  });

  test('include folders are cleaned through the Custom item; invalid ones are rejected', async ({ app, sandbox, server }) => {
    const scratch = path.join(sandbox.home, 'scratch');
    await app.getByTestId('tab-settings').click();

    // a protected folder is refused with a readable error
    await app.getByTestId('include-path').fill('~/Documents');
    await app.getByTestId('include-add').click();
    await expect(app.getByTestId('error-banner')).toContainText('protected');

    await app.getByTestId('include-path').fill(scratch);
    await app.getByTestId('include-mask').fill('*.tmp');
    await app.getByTestId('include-add').click();
    await expect(app.getByTestId('include-list')).toContainText('*.tmp');
    await callApi(server, 'settings.set', { selectedRules: ['custom.include'] });
    // fixture files for the custom folder
    mkdirSync(path.join(scratch, 'sub'), { recursive: true });
    writeFileSync(path.join(scratch, 'a.tmp'), 'x'.repeat(2000));
    writeFileSync(path.join(scratch, 'sub', 'b.tmp'), 'x'.repeat(1000));
    writeFileSync(path.join(scratch, 'keep.txt'), 'keep');

    await app.getByTestId('tab-clean').click();
    await app.getByTestId('group-expand-custom').click();
    await expect(app.getByTestId('rule-custom.include')).toBeChecked();
    await app.getByTestId('btn-analyze').click();
    await expect(app.getByTestId('result-custom.include-size')).toHaveText('2.9 KB');
    await app.getByTestId('btn-clean').click();
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('clean-summary')).toBeVisible();
    expect(existsSync(path.join(scratch, 'a.tmp'))).toBe(false);
    expect(existsSync(path.join(scratch, 'sub', 'b.tmp'))).toBe(false);
    expect(existsSync(path.join(scratch, 'keep.txt'))).toBe(true);
  });
});
