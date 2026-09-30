/**
 * Smart Cleaning settings, Run at startup, Scheduled cleaning and About against a throwaway
 * machine: real files under a temp HOME, fake `systemctl` / `crontab` / `xdg-open` executables.
 * Runs at 380 and 320 px.
 */
import { test as base, expect, type Page } from '@playwright/test';
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { callApi, noHorizontalScroll } from './fixtures';
import { bootScheduleMachine, type Options, type ScheduleMachine } from './helpers/scheduleMachine';

base.skip(process.platform !== 'linux', 'uses shell-script fakes and the Linux layout');

interface Fixtures {
  boot: (opts?: Options) => Promise<ScheduleMachine>;
  errors: string[];
}

const test = base.extend<Fixtures>({
  errors: async ({ page }, use) => {
    const errors: string[] = [];
    page.on('console', (msg) => {
      if (msg.type() === 'error' && !/status of (4|5)\d\d/.test(msg.text())) errors.push(`console.error: ${msg.text()}`);
    });
    page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
    await use(errors);
    expect(errors, 'no console errors / page errors').toEqual([]);
  },
  boot: async ({ errors }, use) => {
    void errors;
    const machines: ScheduleMachine[] = [];
    await use(async (opts) => {
      const m = await bootScheduleMachine(opts);
      machines.push(m);
      return m;
    });
    for (const m of machines) await m.stop();
  },
});

async function openSettings(page: Page, m: ScheduleMachine): Promise<void> {
  await page.goto(m.server.url);
  await expect(page.getByTestId('tabbar')).toBeVisible();
  await page.getByTestId('tab-settings').click();
  await expect(page.getByTestId('page-settings')).toBeVisible();
}

async function fits(page: Page): Promise<void> {
  const m = await noHorizontalScroll(page);
  expect(m.scrollWidth, 'no horizontal scroll').toBeLessThanOrEqual(m.innerWidth);
}

function timerFiles(m: ScheduleMachine): string[] {
  return existsSync(m.unitDir) ? readdirSync(m.unitDir).filter((f) => f.endsWith('.timer')) : [];
}

// ---------------------------------------------------------------- Smart Cleaning

test.describe('Smart Cleaning settings', () => {
  test('every control persists across a reload', async ({ page, boot }) => {
    const m = await boot();
    await openSettings(page, m);
    const card = page.getByTestId('settings-smart');
    await expect(card).toContainText('Smart Cleaning');
    await expect(page.getByTestId('smart-agent-status')).toContainText('Not running — enable Run at startup or keep ClearSweep open');

    await page.getByTestId('smart-enabled').click();
    await expect(page.getByTestId('smart-enabled')).toHaveAttribute('aria-checked', 'true');
    await page.getByTestId('smart-threshold-1000').click();
    await expect(page.getByTestId('smart-threshold-1000')).toHaveAttribute('aria-pressed', 'true');
    await page.getByTestId('smart-threshold-custom').fill('750');
    await page.getByTestId('smart-threshold-custom').press('Enter');
    await expect(page.getByTestId('smart-threshold-1000')).toHaveAttribute('aria-pressed', 'false');
    await page.getByTestId('smart-notify').click();
    await page.getByTestId('smart-auto-clean').click();
    await page.getByTestId('smart-browser-google-chrome').click();
    await expect(page.getByTestId('smart-browser-google-chrome')).toBeChecked();
    await page.getByTestId('smart-browser-mozilla-firefox').click();
    await expect(page.getByTestId('smart-browser-mozilla-firefox')).toBeChecked();
    await page.getByTestId('smart-interval').selectOption('360');
    await fits(page);

    // The server has it all.
    await expect
      .poll(async () => (await callApi<{ smart: Record<string, unknown> }>(m.server, 'settings.get')).smart)
      .toEqual({
        enabled: true,
        thresholdMb: 750,
        cleanOnBrowserClose: ['Google Chrome', 'Mozilla Firefox'],
        autoClean: true,
        notify: false,
        checkIntervalMinutes: 360,
        enforceSleepMinutes: 15,
      });

    await page.reload();
    await page.getByTestId('tab-settings').click();
    await expect(page.getByTestId('smart-enabled')).toHaveAttribute('aria-checked', 'true');
    await expect(page.getByTestId('smart-threshold-custom')).toHaveValue('750');
    await expect(page.getByTestId('smart-notify')).toHaveAttribute('aria-checked', 'false');
    await expect(page.getByTestId('smart-auto-clean')).toHaveAttribute('aria-checked', 'true');
    await expect(page.getByTestId('smart-browser-google-chrome')).toBeChecked();
    await expect(page.getByTestId('smart-browser-mozilla-firefox')).toBeChecked();
    await expect(page.getByTestId('smart-browser-microsoft-edge')).not.toBeChecked();
    await expect(page.getByTestId('smart-interval')).toHaveValue('360');
    await fits(page);
  });

  test('an invalid custom threshold is refused', async ({ page, boot }) => {
    const m = await boot();
    await openSettings(page, m);
    await page.getByTestId('smart-threshold-custom').fill('0');
    await page.getByTestId('smart-threshold-custom').press('Enter');
    await expect(page.getByTestId('smart-threshold-error')).toContainText('whole number of megabytes');
    expect((await callApi<{ smart: { thresholdMb: number } }>(m.server, 'settings.get')).smart.thresholdMb).toBe(500);
    await fits(page);
  });

  test('the agent status is "not running" when nothing runs the agent', async ({ page, boot }) => {
    const m = await boot();
    await openSettings(page, m);
    // The UI line is covered above; the API agrees.
    const status = await callApi<{ running: boolean }>(m.server, 'smart_cleaning.status');
    expect(status.running).toBe(false);
  });
});

// ---------------------------------------------------------------- Startup & Background

test.describe('Startup & Background', () => {
  test('Run at startup installs and removes the autostart entry; Close to tray is desktop only', async ({ page, boot }) => {
    const m = await boot();
    await openSettings(page, m);
    const entry = path.join(m.autostartDir, 'clearsweep-agent.desktop');
    await expect(page.getByTestId('settings-startup')).toContainText('Startup & Background');
    await expect(page.getByTestId('setting-close-to-tray')).toHaveCount(0);
    expect(existsSync(entry)).toBe(false);

    await page.getByTestId('setting-run-at-startup').click();
    await expect(page.getByTestId('setting-run-at-startup')).toHaveAttribute('aria-checked', 'true');
    await expect.poll(() => existsSync(entry)).toBe(true);
    const text = readFileSync(entry, 'utf8');
    expect(text).toMatch(/^Exec=.*clearsweep agent$/m);
    expect(text).toContain('X-GNOME-Autostart-enabled=true');
    await fits(page);

    await page.reload();
    await page.getByTestId('tab-settings').click();
    await expect(page.getByTestId('setting-run-at-startup')).toHaveAttribute('aria-checked', 'true');

    await page.getByTestId('setting-run-at-startup').click();
    await expect(page.getByTestId('setting-run-at-startup')).toHaveAttribute('aria-checked', 'false');
    await expect.poll(() => existsSync(entry)).toBe(false);
  });
});

// ---------------------------------------------------------------- Language and About

test.describe('Language and About', () => {
  test('render, and Open data folder launches the file manager', async ({ page, boot }) => {
    const m = await boot();
    await openSettings(page, m);
    await expect(page.getByTestId('setting-language')).toHaveValue('en');
    await expect(page.getByTestId('setting-language').locator('option')).toHaveText(['English']);
    await expect(page.getByTestId('tab-settings')).toContainText('Settings');

    await page.getByTestId('settings-about').scrollIntoViewIfNeeded();
    await expect(page.getByTestId('about-name')).toHaveText('ClearSweep');
    await expect(page.getByTestId('about-version')).toHaveText(/^version \d+\.\d+\.\d+/);
    await expect(page.getByTestId('about-promise')).toHaveText('All features are free. No account, no ads, no telemetry.');
    await expect(page.getByTestId('about-license')).toContainText('MIT');
    await expect(page.getByTestId('about-data-dir')).toContainText(m.dataDir);
    await fits(page);

    await page.getByTestId('about-open-data').click();
    await expect.poll(() => m.log().filter((l) => l.startsWith('xdg-open'))).toEqual([`xdg-open ${m.dataDir}`]);
    await expect(page.getByTestId('about-open-error')).toHaveCount(0);
  });

  test('Open data folder explains when no file manager is available', async ({ page, boot }) => {
    const m = await boot({ tools: [] });
    await openSettings(page, m);
    await page.getByTestId('about-open-data').click();
    await expect(page.getByTestId('about-open-error')).toContainText('xdg-open');
    await fits(page);
  });
});

// ---------------------------------------------------------------- Scheduled cleaning

test.describe('Scheduled cleaning', () => {
  test('add, edit, disable, run now and delete a schedule, with the systemd units on disk', async ({ page, boot }) => {
    const m = await boot();
    await callApi(m.server, 'settings.set', { selectedRules: ['chrome.cache'] });
    await openSettings(page, m);
    await expect(page.getByTestId('settings-schedules')).toContainText('Scheduled Cleaning');
    await page.getByTestId('settings-schedules-link').click();
    await expect(page.getByTestId('page-schedules')).toBeVisible();
    await expect(page.getByTestId('appbar-title')).toHaveText('Scheduled cleaning');
    await expect(page.getByTestId('schedules-backend')).toContainText('systemd user timers');
    await expect(page.getByText('No schedules yet')).toBeVisible();

    // Add: weekly on Monday and Wednesday at 03:00.
    await page.getByTestId('schedule-add').click();
    await expect(page.getByTestId('schedule-sheet')).toBeVisible();
    await page.getByTestId('schedule-name').fill('Nightly');
    await page.getByTestId('schedule-frequency').selectOption('weekly');
    await page.getByTestId('schedule-weekday-3').click();
    await page.getByTestId('schedule-time').fill('03:00');
    await fits(page);
    await page.getByTestId('schedule-save').click();
    await expect(page.getByTestId('schedule-sheet')).toHaveCount(0);

    const [timerName] = timerFiles(m);
    expect(timerName).toMatch(/^clearsweep-[0-9a-f]{16}\.timer$/);
    const id = timerName!.slice('clearsweep-'.length, -'.timer'.length);
    await expect(page.getByTestId(`schedule-when-${id}`)).toHaveText('Every Mon and Wed at 03:00');
    await expect(page.getByTestId(`schedule-last-${id}`)).toHaveText('Has not run yet');
    const timerPath = path.join(m.unitDir, timerName!);
    expect(readFileSync(timerPath, 'utf8')).toContain('OnCalendar=Mon,Wed *-*-* 03:00:00');
    expect(readFileSync(timerPath, 'utf8')).toContain('Persistent=true');
    const service = readFileSync(path.join(m.unitDir, `clearsweep-${id}.service`), 'utf8');
    expect(service).toMatch(new RegExp(`ExecStart="[^"]*clearsweep" "clean" "--auto" "--source" "scheduled" "--schedule" "${id}"`));
    expect(m.log()).toContain(`systemctl --user enable --now clearsweep-${id}.timer`);
    await fits(page);

    // A second schedule cannot reuse the name.
    await page.getByTestId('schedule-add').click();
    await page.getByTestId('schedule-name').fill('nightly');
    await page.getByTestId('schedule-save').click();
    await expect(page.getByTestId('schedule-problem')).toContainText('already a schedule named');
    await fits(page);
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('schedule-sheet')).toHaveCount(0);

    // Edit: Friday too, at 04:15.
    await page.getByTestId(`schedule-edit-${id}`).click();
    await expect(page.getByTestId('schedule-name')).toHaveValue('Nightly');
    await page.getByTestId('schedule-weekday-5').click();
    await page.getByTestId('schedule-time').fill('04:15');
    await page.getByTestId('schedule-save').click();
    await expect(page.getByTestId('schedule-sheet')).toHaveCount(0);
    await expect(page.getByTestId(`schedule-when-${id}`)).toHaveText('Every Mon, Wed and Fri at 04:15');
    expect(readFileSync(timerPath, 'utf8')).toContain('OnCalendar=Mon,Wed,Fri *-*-* 04:15:00');
    expect(m.log()).toContain(`systemctl --user restart clearsweep-${id}.timer`);

    // Disable and enable.
    await page.getByTestId(`schedule-toggle-${id}`).click();
    await expect(page.getByTestId(`schedule-toggle-${id}`)).toHaveAttribute('aria-checked', 'false');
    expect(m.log()).toContain(`systemctl --user disable --now clearsweep-${id}.timer`);
    await page.getByTestId(`schedule-toggle-${id}`).click();
    await expect(page.getByTestId(`schedule-toggle-${id}`)).toHaveAttribute('aria-checked', 'true');

    // Run now: the Chrome cache goes, the result is shown and survives a reload.
    expect(existsSync(m.chromeCacheFile)).toBe(true);
    await page.getByTestId(`schedule-run-${id}`).click();
    await expect(page.getByTestId('schedules-note')).toContainText('Nightly: cleaned 300 KB');
    expect(existsSync(m.chromeCacheFile)).toBe(false);
    await expect(page.getByTestId(`schedule-last-${id}`)).toContainText('cleaned 300 KB');
    await page.reload();
    await expect(page.getByTestId(`schedule-last-${id}`)).toContainText('cleaned 300 KB');
    const history = await callApi<{ source: string }[]>(m.server, 'cleaner.history', { limit: 1 });
    expect(history[0]!.source).toBe('scheduled');
    await fits(page);

    // Delete needs a confirmation and removes every file.
    await page.getByTestId(`schedule-delete-${id}`).click();
    await expect(page.getByRole('dialog')).toContainText('Delete "Nightly"?');
    await page.getByRole('button', { name: 'Cancel' }).click();
    expect(existsSync(timerPath)).toBe(true);
    await page.getByTestId(`schedule-delete-${id}`).click();
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(page.getByText('No schedules yet')).toBeVisible();
    expect(existsSync(timerPath)).toBe(false);
    expect(existsSync(path.join(m.unitDir, `clearsweep-${id}.service`))).toBe(false);
    expect(m.log()).toContain(`systemctl --user disable --now clearsweep-${id}.timer`);
  });

  test('falls back to crontab without a systemd user manager and keeps the other lines', async ({ page, boot }) => {
    // Only `crontab` on PATH: no systemctl at all.
    const m = await boot({
      tools: ['crontab'],
      setup: (mm) => {
        writeFileSync(path.join(mm.state, 'crontab'), '# mine\n*/5 * * * * /usr/bin/true\n');
      },
    });
    await openSettings(page, m);
    await page.getByTestId('settings-schedules-link').click();
    await expect(page.getByTestId('schedules-backend')).toContainText('cron');

    await page.getByTestId('schedule-add').click();
    await page.getByTestId('schedule-name').fill('Daily');
    await page.getByTestId('schedule-time').fill('04:15');
    await page.getByTestId('schedule-save').click();
    await expect(page.getByTestId('schedule-sheet')).toHaveCount(0);
    const cron = m.readState('crontab');
    expect(cron.startsWith('# mine\n*/5 * * * * /usr/bin/true\n')).toBe(true);
    expect(cron).toMatch(/^15 4 \* \* \* \S*clearsweep clean --auto --source scheduled --schedule ([0-9a-f]{16}) # clearsweep-schedule:\1$/m);
    expect(existsSync(m.unitDir)).toBe(false);
    await fits(page);

    // Deleting restores the original crontab.
    await page.getByRole('button', { name: 'Delete' }).click();
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(page.getByText('No schedules yet')).toBeVisible();
    expect(m.readState('crontab')).toBe('# mine\n*/5 * * * * /usr/bin/true\n');
  });

  test('says so when no OS scheduler is available, and still saves schedules', async ({ page, boot }) => {
    const m = await boot({ tools: [] });
    await openSettings(page, m);
    await page.getByTestId('settings-schedules-link').click();
    await expect(page.getByTestId('schedules-backend')).toContainText('Neither systemd user services nor cron');
    await page.getByTestId('schedule-add').click();
    await page.getByTestId('schedule-name').fill('Quiet');
    await page.getByTestId('schedule-save').click();
    await expect(page.getByTestId('schedule-sheet')).toHaveCount(0);
    await expect(page.getByText('Every day at 03:00')).toBeVisible();
    expect(timerFiles(m)).toEqual([]);
    await fits(page);
  });

  test('monthly, hourly and login schedules read naturally', async ({ page, boot }) => {
    const m = await boot();
    await openSettings(page, m);
    await page.getByTestId('settings-schedules-link').click();
    const add = async (name: string, freq: string, extra?: () => Promise<void>) => {
      await page.getByTestId('schedule-add').click();
      await page.getByTestId('schedule-name').fill(name);
      await page.getByTestId('schedule-frequency').selectOption(freq);
      await extra?.();
      await page.getByTestId('schedule-save').click();
      await expect(page.getByTestId('schedule-sheet')).toHaveCount(0);
    };
    await add('Monthly', 'monthly', async () => {
      await page.getByTestId('schedule-dom').selectOption('15');
    });
    await add('Hourly', 'hourly', async () => {
      await page.getByTestId('schedule-minute').fill('20');
    });
    await add('Login', 'on_login');
    await expect(page.getByText('On day 15 of every month at 03:00')).toBeVisible();
    await expect(page.getByText('Every hour, 20 minutes past')).toBeVisible();
    await expect(page.getByText('Every time you log in')).toBeVisible();
    // On-login schedules are autostart entries; the others are timers.
    expect(readdirSync(m.autostartDir).filter((f) => f.startsWith('clearsweep-schedule-'))).toHaveLength(1);
    expect(timerFiles(m)).toHaveLength(2);
    const timers = timerFiles(m).map((f) => readFileSync(path.join(m.unitDir, f), 'utf8'));
    expect(timers.some((t) => t.includes('OnCalendar=*-*-15 03:00:00'))).toBe(true);
    expect(timers.some((t) => t.includes('OnCalendar=*-*-* *:20:00'))).toBe(true);
    await fits(page);
  });
});
