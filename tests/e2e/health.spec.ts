/**
 * Health Check on the Home tab, against the fake machine (real files, browser databases,
 * temp files). Runs at 380 and 320 px. Everything the tests assert about deletion is checked
 * on disk or through the cookie API, never only in the UI.
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import type { Page } from '@playwright/test';
import { callApi, expect, noHorizontalScroll, resetSandbox, test as base } from './fixtures';
import { bootMachine, type MachineOptions, type StartupMachine } from './helpers/startupMachine';

base.skip(process.platform !== 'linux', 'the fake machine uses the Linux layout');

interface CookieDomain {
  domain: string;
  count: number;
  browsers: string[];
  kept: boolean;
}

interface Report {
  score: number | null;
  categories: { id: string; status: string; summary: string; metrics: Record<string, number> }[];
}

const test = base.extend<{
  boot: (setup: (m: StartupMachine) => void, opts?: MachineOptions) => Promise<StartupMachine>;
}>({
  boot: async ({}, use) => {
    const machines: StartupMachine[] = [];
    await use(async (setup, opts) => {
      const m = await bootMachine(setup, opts);
      machines.push(m);
      return m;
    });
    for (const m of machines) await m.stop();
  },
});

async function fits(page: Page): Promise<void> {
  const m = await noHorizontalScroll(page);
  expect(m.scrollWidth, 'no horizontal scroll').toBeLessThanOrEqual(m.innerWidth);
}

/** The number in "Health score 72 of 100". */
async function ringScore(page: Page): Promise<number> {
  const label = (await page.getByTestId('health-ring').getAttribute('aria-label')) ?? '';
  const m = /Health score (\d+) of 100/.exec(label);
  expect(m, `ring label "${label}"`).not.toBeNull();
  return Number(m![1]);
}

async function scan(page: Page): Promise<void> {
  await page.getByTestId('btn-scan').click();
  await expect(page.getByTestId('cat-security')).toBeVisible();
  await expect(page.getByTestId('btn-scan')).toBeEnabled();
  await expect(page.getByTestId('health-progress')).toHaveCount(0);
}

const KEEP = ['google.com', 'github.com'];

async function cookieDomains(server: Parameters<typeof callApi>[0]): Promise<CookieDomain[]> {
  return callApi<CookieDomain[]>(server, 'cookies.list');
}

test.describe('health check', () => {
  test('a first visit invites a scan; the scan shows four categories with the fixture junk and changes nothing', async ({
    app,
    server,
    sandbox,
  }) => {
    await callApi(server, 'settings.set', { cookieKeep: KEEP });
    await app.getByTestId('tab-home').click();
    await expect(app.getByTestId('page-home')).toBeVisible();
    await expect(app.getByRole('img', { name: 'Health score not measured yet' })).toBeVisible();
    await expect(app.getByTestId('health-status')).toHaveText('Not scanned yet');
    await expect(app.getByTestId('btn-scan')).toHaveText('Scan');
    await expect(app.getByTestId('btn-fix-all')).toHaveCount(0);
    await expect(app.getByTestId('quick-links')).toBeVisible();
    await fits(app);

    const data0 = path.join(sandbox.chromeCache(), 'Cache/Cache_Data/data_0');
    const cookies = path.join(sandbox.chromeData(), 'Network/Cookies');
    const before = readFileSync(cookies);
    await scan(app);

    for (const id of ['privacy', 'space', 'speed', 'security']) await expect(app.getByTestId(`cat-${id}`)).toBeVisible();
    await expect(app.locator('[data-testid^="cat-"][data-status]')).toHaveCount(4);
    // junk from the fixture: non-zero bytes and files
    const space = app.getByTestId('cat-space-summary');
    await expect(space).toHaveText(/^[\d.]+ (KB|MB|GB) of junk in [1-9]\d* files?\.$/);
    await expect(app.getByTestId('cat-space')).toHaveAttribute('data-status', 'warning');
    // tracking cookies: everything off the keep list, in several browsers
    await expect(app.getByTestId('cat-privacy-summary')).toHaveText(/[1-9]\d* tracking cookies in [2-9] browsers/);
    // nothing runs at startup on the fake machine
    await expect(app.getByTestId('cat-speed')).toHaveAttribute('data-status', 'good');
    // no package manager on the fake machine's PATH: unavailable, not an error
    await expect(app.getByTestId('cat-security')).toHaveAttribute('data-status', 'unavailable');
    await expect(app.getByTestId('cat-security-summary')).toContainText('No supported package manager');
    await expect(app.getByTestId('error-banner')).toHaveCount(0);
    const score = await ringScore(app);
    expect(score).toBeGreaterThan(0);
    expect(score).toBeLessThan(100);
    await expect(app.getByTestId('health-status')).toContainText('need');
    await fits(app);

    // the space sheet lists the biggest areas
    await app.getByTestId('cat-space').click();
    await expect(app.getByTestId('health-sheet')).toBeVisible();
    await expect(app.getByTestId('sel-space')).toBeChecked();
    await expect(app.getByTestId('sheet-junk')).toContainText('Google Chrome');
    await fits(app);
    await app.getByTestId('health-sheet-done').click();

    // scanning is read-only
    expect(existsSync(data0)).toBe(true);
    expect(readFileSync(cookies).equals(before)).toBe(true);
    const domains = await cookieDomains(server);
    expect(domains.some((d) => d.domain === 'tracker.example')).toBe(true);

    // the API agrees with what the page shows
    const api = await callApi<Report>(server, 'health.analyze');
    const sp = api.categories.find((c) => c.id === 'space')!;
    expect(sp.metrics.bytes).toBeGreaterThan(0);
    expect(sp.metrics.files).toBeGreaterThan(0);
    expect(api.score).toBe(score);
  });

  test('the previous score is there immediately after a reload', async ({ app }) => {
    await app.getByTestId('tab-home').click();
    await scan(app);
    const score = await ringScore(app);
    await app.reload();
    await expect(app.getByTestId('page-home')).toBeVisible();
    await expect(app.getByRole('img', { name: `Health score ${score} of 100` })).toBeVisible();
    await expect(app.getByTestId('health-scanned')).toContainText('Last scan');
    await expect(app.getByTestId('health-stale')).toBeVisible();
    await expect(app.getByTestId('btn-scan')).toHaveText('Rescan');
    // an old result cannot be fixed
    await expect(app.getByTestId('btn-fix-all')).toHaveCount(0);
    await fits(app);
  });

  test('Fix all with space and privacy selected deletes the junk, keeps the keep-listed cookies and raises the score', async ({
    app,
    server,
    sandbox,
  }) => {
    await callApi(server, 'settings.set', { cookieKeep: KEEP });
    const tmp = path.join(sandbox.dir, 'root', 'tmp');
    const junk = [
      path.join(sandbox.chromeCache(), 'Cache/Cache_Data/data_0'),
      path.join(sandbox.chromeCache(), 'Code Cache/js/index'),
      path.join(sandbox.chromeCache('Profile 1'), 'Cache/Cache_Data/data_0'),
      path.join(tmp, 'old-1.tmp'),
    ];
    const keep = [
      path.join(tmp, 'new.tmp'),
      path.join(sandbox.chromeData(), 'Bookmarks'),
      path.join(sandbox.chromeData(), 'Preferences'),
      path.join(sandbox.chromeData(), 'Network/Cookies'),
      path.join(sandbox.chromeData(), 'History'),
    ];
    for (const f of [...junk, ...keep]) expect(existsSync(f), f).toBe(true);
    const beforeDomains = await cookieDomains(server);
    expect(beforeDomains.filter((d) => !d.kept).length).toBeGreaterThan(0);

    await app.getByTestId('tab-home').click();
    await scan(app);
    const before = await ringScore(app);

    await app.getByTestId('btn-fix-all').click();
    const sheet = app.getByTestId('confirm-sheet');
    await expect(sheet).toBeVisible();
    // exactly the two selected parts are named
    await expect(sheet).toContainText('junk');
    await expect(sheet).toContainText('tracking cookies');
    await expect(sheet).not.toContainText('startup');
    await expect(sheet).not.toContainText('update');
    await expect(sheet).not.toContainText('sleep');
    await fits(app);
    // nothing is deleted before the user confirms
    expect(existsSync(junk[0]!)).toBe(true);
    await sheet.getByTestId('confirm-sheet-confirm').click();

    await expect(app.getByTestId('health-result')).toBeVisible();
    await expect(app.getByTestId('health-result-title')).toHaveText('All done');
    await expect(app.getByTestId('done-space')).toContainText('Deleted');
    await expect(app.getByTestId('done-privacy')).toContainText('Removed');
    await expect(app.getByTestId('done-startup')).toHaveCount(0);
    await expect(app.getByTestId('done-updates')).toHaveCount(0);
    await fits(app);

    // on disk
    for (const f of junk) expect(existsSync(f), `${f} was junk`).toBe(false);
    for (const f of keep) expect(existsSync(f), `${f} must survive`).toBe(true);
    const after = await cookieDomains(server);
    expect(after.filter((d) => !d.kept)).toEqual([]);
    const kept = after.map((d) => d.domain).sort();
    expect(kept).toContain('google.com');
    expect(kept).toContain('github.com');
    expect(kept).not.toContain('tracker.example');
    expect(kept).not.toContain('notgoogle.com');
    // the score went up and the categories show the fresh state
    const now = await ringScore(app);
    expect(now).toBeGreaterThan(before);
    await expect(app.getByTestId('health-result-score')).toHaveText(`Score ${before} → ${now}`);
    await expect(app.getByTestId('cat-space')).toHaveAttribute('data-status', 'good');
    await expect(app.getByTestId('cat-privacy')).toHaveAttribute('data-status', 'good');
    await fits(app);
    // and the new state is what a reload shows
    await app.reload();
    await expect(app.getByRole('img', { name: `Health score ${now} of 100` })).toBeVisible();
  });

  test('what is unticked in a sheet is left alone', async ({ app, server, sandbox }) => {
    await callApi(server, 'settings.set', { cookieKeep: KEEP });
    const cacheFile = path.join(sandbox.chromeCache(), 'Cache/Cache_Data/data_0');
    await app.getByTestId('tab-home').click();
    await scan(app);
    await app.getByTestId('cat-privacy').click();
    await app.getByTestId('sel-privacy').uncheck();
    await app.getByTestId('health-sheet-done').click();
    await app.getByTestId('btn-fix-all').click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('junk');
    await expect(app.getByTestId('confirm-sheet')).not.toContainText('tracking cookies');
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('health-result')).toBeVisible();
    await expect(app.getByTestId('done-privacy')).toHaveCount(0);
    expect(existsSync(cacheFile)).toBe(false);
    // tracking cookies were not touched
    const domains = await cookieDomains(server);
    expect(domains.some((d) => d.domain === 'tracker.example')).toBe(true);
    await expect(app.getByTestId('cat-privacy')).toHaveAttribute('data-status', 'warning');
    await fits(app);

    // with everything unticked there is nothing to confirm
    await app.getByTestId('cat-privacy').click();
    await app.getByTestId('sel-privacy').uncheck();
    await app.getByTestId('health-sheet-done').click();
    await expect(app.getByTestId('btn-fix-all')).toBeDisabled();
  });

  test('startup items and background apps are listed, ticked, and only those are changed; a running browser is reported', async ({
    boot,
    page,
  }) => {
    const slackDesktop = '[Desktop Entry]\nType=Application\nName=Slack\nExec=/opt/Slack/slack -u %U\n';
    const other = '[Desktop Entry]\nType=Application\nName=Other Tool\nExec=/opt/other/other\n';
    const m = await boot(
      (mm) => {
        resetSandbox(mm.server.dir);
        mkdirSync(mm.autostartDir, { recursive: true });
        writeFileSync(path.join(mm.autostartDir, 'slack.desktop'), slackDesktop);
        writeFileSync(path.join(mm.autostartDir, 'other.desktop'), other);
      },
      // Slack is heavy (300 MB), Chrome is running, "Other Tool" is not running at all.
      { processes: 'chrome,slack|/opt/Slack/slack|300|0' },
    );
    await page.goto(m.server.url);
    await expect(page.getByTestId('page-home')).toBeVisible();
    await scan(page);

    await expect(page.getByTestId('cat-speed')).toHaveAttribute('data-status', 'problem');
    await page.getByTestId('cat-speed').click();
    await expect(page.getByTestId('sel-startup-xdg:user:slack.desktop')).toBeChecked();
    await expect(page.getByTestId('sel-app-slack')).toBeChecked();
    // "Other Tool" is not running, so it has no measurable impact: not offered
    await expect(page.getByTestId('sel-startup-xdg:user:other.desktop')).toHaveCount(0);
    await fits(page);
    await page.getByTestId('health-sheet-done').click();

    await page.getByTestId('btn-fix-all').click();
    const sheet = page.getByTestId('confirm-sheet');
    await expect(sheet).toContainText('disable 1 startup item');
    await expect(sheet).toContainText('put 1 app to sleep');
    await sheet.getByTestId('confirm-sheet-confirm').click();
    await expect(page.getByTestId('health-result')).toBeVisible();

    // startup + sleep: Slack's entry is switched off, the other one is untouched
    expect(readFileSync(path.join(m.autostartDir, 'slack.desktop'), 'utf8')).toContain('Hidden=true');
    expect(readFileSync(path.join(m.autostartDir, 'other.desktop'), 'utf8')).toBe(other);
    await expect(page.getByTestId('done-startup')).toContainText('Switched off 1 of 1 startup item');
    await expect(page.getByTestId('done-sleep')).toContainText('Put 1 of 1 app to sleep');

    // Chrome is running: its cookies and cache were left alone and the user is asked
    const chromeData = path.join(m.home, '.config/google-chrome/Default');
    const chromeCache = path.join(m.home, '.cache/google-chrome/Default/Cache/Cache_Data/data_0');
    expect(existsSync(chromeCache)).toBe(true);
    expect(existsSync(path.join(chromeData, 'Network/Cookies'))).toBe(true);
    // (Slack is "running" too, so its cache is skipped as well)
    await expect(page.getByTestId('confirm-sheet')).toContainText(/Google Chrome, Slack are running/);
    await expect(page.getByTestId('done-privacy')).toContainText('Google Chrome still running');
    await fits(page);
    await page.getByRole('button', { name: 'Skip' }).click();
    await expect(page.getByTestId('confirm-sheet')).toHaveCount(0);
    await expect(page.getByTestId('btn-close-and-fix')).toBeVisible();
    await expect(page.getByTestId('health-result-title')).toHaveText('Done, with some issues');
    await fits(page);
    // the browsers that were not running were cleaned
    expect(existsSync(path.join(m.home, '.cache/microsoft-edge/Default/Cache/Cache_Data/data_0'))).toBe(false);
  });

  test('quick links open Clean, Performance and the Software Updater', async ({ app }) => {
    await app.getByTestId('tab-home').click();
    await app.getByTestId('link-clean').click();
    await expect(app.getByTestId('page-clean')).toBeVisible();
    await app.getByTestId('tab-home').click();
    await app.getByTestId('link-performance').click();
    await expect(app.getByTestId('page-performance')).toBeVisible();
    await app.getByTestId('tab-home').click();
    await app.getByTestId('link-updater').click();
    await expect(app.getByTestId('page-updater')).toBeVisible();
    await fits(app);
  });
});
