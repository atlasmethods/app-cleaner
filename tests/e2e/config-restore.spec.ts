/**
 * Config Issues (scan -> fix -> restore) on fixture files, and System Restore against fake
 * `timeshift` / `snapper` executables (the server's PATH contains nothing else). Runs at 380 and
 * 320 px.
 */
import { test as base, expect, type Page } from '@playwright/test';
import { existsSync, lstatSync, mkdirSync, readFileSync, readlinkSync, symlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { noHorizontalScroll } from './fixtures';
import { startConfigMachine, type ConfigMachine } from './helpers/configMachine';

base.skip(process.platform !== 'linux', 'uses shell-script fakes and the Linux layout');

interface Fixtures {
  machine: ConfigMachine;
  bare: ConfigMachine;
  app: Page;
}

const test = base.extend<Fixtures>({
  machine: async ({}, use) => {
    const m = await startConfigMachine({ tools: true });
    try {
      await use(m);
    } finally {
      await m.stop();
    }
  },
  bare: async ({}, use) => {
    const m = await startConfigMachine({ tools: false });
    try {
      await use(m);
    } finally {
      await m.stop();
    }
  },
  app: async ({ page, machine }, use) => {
    await use(await open(page, machine));
  },
});

async function open(page: Page, machine: ConfigMachine): Promise<Page> {
  const errors: string[] = [];
  page.on('console', (msg) => {
    if (msg.type() === 'error') errors.push(`console.error: ${msg.text()}`);
  });
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
  page.on('response', (r) => {
    if (r.status() >= 400) errors.push(`http ${r.status()}: ${r.url()}`);
  });
  await page.goto(machine.server.url);
  await expect(page.getByTestId('tabbar')).toBeVisible();
  (page as Page & { __errors?: string[] }).__errors = errors;
  return page;
}

test.afterEach(async ({ page }) => {
  const errors = (page as Page & { __errors?: string[] }).__errors ?? [];
  expect(errors, 'no console errors / page errors').toEqual([]);
});

async function openTool(app: Page, id: string): Promise<void> {
  await app.getByTestId('tab-tools').click();
  await app.getByTestId(`tile-${id}`).click();
  await expect(app.getByTestId(`page-${id}`)).toBeVisible();
}

async function fits(app: Page): Promise<void> {
  const m = await noHorizontalScroll(app);
  expect(m.scrollWidth, 'no horizontal scroll').toBeLessThanOrEqual(m.innerWidth);
}

function write(file: string, content: string): void {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, content);
}

const BROKEN = '[Desktop Entry]\nType=Application\nName=Old App\nExec=/opt/vanished/app --flag %U\n';
const MIME = '# my associations\n[Default Applications]\ntext/plain=good.desktop;vanished.desktop;\n\n[Removed Associations]\ntext/x=vanished.desktop;\n';

interface Files {
  launcher: string;
  good: string;
  autostart: string;
  link: string;
  mimeapps: string;
}

function makeFiles(m: ConfigMachine): Files {
  const f: Files = {
    launcher: path.join(m.home, '.local/share/applications/oldapp.desktop'),
    good: path.join(m.home, '.local/share/applications/good.desktop'),
    autostart: path.join(m.configDir, 'autostart/old-auto.desktop'),
    link: path.join(m.home, '.local/bin/dead'),
    mimeapps: path.join(m.configDir, 'mimeapps.list'),
  };
  write(path.join(m.root, 'usr/bin/present'), '#!/bin/sh\n');
  write(f.launcher, BROKEN);
  write(f.good, '[Desktop Entry]\nType=Application\nName=Good\nExec=/usr/bin/present\n');
  write(f.autostart, BROKEN.replace('Old App', 'Old Autostart'));
  mkdirSync(path.dirname(f.link), { recursive: true });
  symlinkSync('/opt/vanished/tool', f.link);
  write(f.mimeapps, MIME);
  return f;
}

async function scanAndFix(app: Page, unticked?: string): Promise<void> {
  await openTool(app, 'registry');
  await expect(app.getByTestId('appbar-title')).toHaveText('Config Issues');
  await fits(app);
  await app.getByTestId('registry-scan').click();
  await expect(app.getByTestId('registry-results')).toBeVisible();
  await expect(app.locator('[data-testid^="issue-check-"]')).toHaveCount(4);
  await expect(app.getByTestId('registry-fix')).toContainText('(4)');
  await fits(app);
  if (unticked) await app.getByTestId(`registry-group-check-${unticked}`).click();
}

test.describe('Config Issues', () => {
  test('scan, fix with a backup, restore byte for byte', async ({ app, machine }) => {
    const f = makeFiles(machine);
    await scanAndFix(app, 'autostart');
    await expect(app.getByTestId('registry-fix')).toContainText('(3)');
    await app.getByTestId('registry-fix').click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('backup');
    await fits(app);
    // nothing is touched before the confirmation
    expect(existsSync(f.launcher)).toBe(true);
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('registry-fix-summary')).toContainText('Fixed 3.');
    await fits(app);

    expect(existsSync(f.launcher)).toBe(false);
    expect(existsSync(f.good)).toBe(true);
    expect(existsSync(f.autostart)).toBe(true); // it was unticked
    expect(() => lstatSync(f.link)).toThrow();
    expect(readFileSync(f.mimeapps, 'utf8')).toBe(
      '# my associations\n[Default Applications]\ntext/plain=good.desktop;\n\n[Removed Associations]\ntext/x=vanished.desktop;\n',
    );
    // the fixed items left the list, the unticked one stayed
    await expect(app.locator('[data-testid^="issue-check-"]')).toHaveCount(1);

    await app.getByTestId('registry-restore-backup').click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('Restore the backup?');
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('registry-note')).toContainText('Restored 3 items');
    expect(readFileSync(f.launcher, 'utf8')).toBe(BROKEN);
    expect(readFileSync(f.mimeapps, 'utf8')).toBe(MIME);
    expect(readlinkSync(f.link)).toBe('/opt/vanished/tool');
    await fits(app);

    // a second scan finds all four problems again
    await app.getByTestId('registry-scan').click();
    await expect(app.locator('[data-testid^="issue-check-"]')).toHaveCount(4);
  });

  test('backups sub-page lists, restores and deletes', async ({ app, machine }) => {
    const f = makeFiles(machine);
    await scanAndFix(app);
    await app.getByTestId('registry-fix').click();
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('registry-fix-summary')).toContainText('Fixed 4.');
    await expect(app.getByTestId('registry-empty')).toBeVisible();

    await app.getByTestId('registry-open-backups').click();
    const rows = app.locator('[data-testid^="backup-row-"]');
    await expect(rows).toHaveCount(1);
    await expect(rows.first()).toContainText('4 items');
    await fits(app);
    await app.locator('[data-testid^="backup-restore-"]').first().click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('Restore this backup?');
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('backups-note')).toContainText('Restored 4 items');
    expect(readFileSync(f.autostart, 'utf8')).toContain('Old Autostart');

    await app.locator('[data-testid^="backup-delete-"]').first().click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('permanently deleted');
    await fits(app);
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('backups-empty')).toBeVisible();
    expect(existsSync(path.join(machine.dataDir, 'backups'))).toBe(true);
    // the restored files are not touched by deleting the backup
    expect(existsSync(f.launcher)).toBe(true);
    await app.getByTestId('backups-back').click();
    await expect(app.getByTestId('registry-scan')).toBeVisible();
  });

  test('cancelling the confirmation changes nothing', async ({ app, machine }) => {
    const f = makeFiles(machine);
    await scanAndFix(app);
    await app.getByTestId('registry-fix').click();
    await app.getByRole('button', { name: 'Cancel' }).click();
    await expect(app.getByTestId('confirm-sheet')).toHaveCount(0);
    expect(existsSync(f.launcher) && existsSync(f.autostart)).toBe(true);
    expect(readFileSync(f.mimeapps, 'utf8')).toBe(MIME);
    expect(existsSync(path.join(machine.dataDir, 'backups'))).toBe(false);
  });

  test('a clean machine reports no issues', async ({ app }) => {
    await openTool(app, 'registry');
    await app.getByTestId('registry-scan').click();
    await expect(app.getByTestId('registry-empty')).toContainText('No issues found');
    await expect(app.getByTestId('registry-fix')).toHaveCount(0);
    await fits(app);
  });
});

test.describe('System Restore', () => {
  test('lists points from Timeshift and Snapper and protects the most recent ones', async ({ app }) => {
    await openTool(app, 'restore');
    await expect(app.getByTestId('restore-list')).toBeVisible();
    await expect(app.locator('[data-testid^="point-timeshift:"]')).toHaveCount(3);
    const newest = 'timeshift:2024-03-01_03-00-00';
    await expect(app.getByTestId(`point-kind-${newest}`)).toHaveText('Timeshift');
    await expect(app.getByTestId(`point-newest-${newest}`)).toHaveText('Most recent');
    await expect(app.getByTestId(`point-delete-${newest}`)).toBeDisabled();
    await expect(app.getByTestId('point-delete-snapper:root:2')).toBeDisabled();
    await expect(app.getByTestId('point-delete-snapper:root:1')).toBeEnabled();
    await expect(app.getByTestId('point-snapper:root:1')).toContainText('first snapper');
    await expect(app.getByTestId('point-timeshift:2024-01-01_10-00-01')).toContainText('Before the big update');
    await fits(app);
  });

  test('creates a restore point and deletes an older one', async ({ app, machine }) => {
    await openTool(app, 'restore');
    await expect(app.getByTestId('restore-create')).toBeDisabled();
    await app.getByTestId('restore-description').fill('E2E point');
    await app.getByTestId('restore-create').click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('E2E point');
    await fits(app);
    expect(machine.log().filter((l) => l.startsWith('timeshift --create'))).toEqual([]);
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('restore-note')).toContainText('Restore point created.');
    expect(machine.log()).toContain('timeshift --create --comments E2E point --tags O --scripted');
    await expect(app.locator('[data-testid^="point-timeshift:"]')).toHaveCount(4);
    // the former newest point is deletable now, the new one is protected
    const created = 'timeshift:2024-06-13_12-00-00';
    await expect(app.getByTestId(`point-newest-${created}`)).toBeVisible();
    await expect(app.getByTestId(`point-delete-${created}`)).toBeDisabled();
    await expect(app.getByTestId('point-delete-timeshift:2024-03-01_03-00-00')).toBeEnabled();

    const old = 'timeshift:2024-01-01_10-00-01';
    await app.getByTestId(`point-delete-${old}`).click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('permanently deleted');
    expect(machine.readState('timeshift.db')).toContain('2024-01-01_10-00-01');
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId(`point-${old}`)).toHaveCount(0);
    expect(machine.readState('timeshift.db')).not.toContain('2024-01-01_10-00-01');
    expect(machine.log()).toContain('timeshift --delete --snapshot 2024-01-01_10-00-01 --yes --scripted');
    await fits(app);

    // snapper: create and delete as well
    await app.getByTestId('point-delete-snapper:root:1').click();
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('point-snapper:root:1')).toHaveCount(0);
    expect(machine.log()).toContain('snapper -c root delete 1');
  });

  test('the server refuses to delete the newest point even when asked directly', async ({ app, machine }) => {
    await openTool(app, 'restore');
    await expect(app.getByTestId('restore-list')).toBeVisible();
    const res = await app.evaluate(async ({ origin, token }) => {
      const r = await fetch(`${origin}/api/call`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Sweep-Token': token },
        body: JSON.stringify({ callId: 'x', method: 'restore.delete_point', params: { id: 'timeshift:2024-03-01_03-00-00' } }),
      });
      return r.text();
    }, { origin: machine.server.origin, token: machine.server.token });
    expect(res).toContain('most recent');
    expect(machine.readState('timeshift.db')).toContain('2024-03-01_03-00-00');
    expect(machine.log().filter((l) => l.startsWith('timeshift --delete'))).toEqual([]);
  });

  test('ClearSweep backups appear and can be restored from here', async ({ app, machine }) => {
    const f = makeFiles(machine);
    await scanAndFix(app);
    await app.getByTestId('registry-fix').click();
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('registry-fix-summary')).toContainText('Fixed 4.');
    expect(existsSync(f.launcher)).toBe(false);

    await app.getByTestId('tab-tools').click();
    await app.getByTestId('tile-restore').click();
    await expect(app.getByTestId('restore-backups')).toBeVisible();
    const restoreBtn = app.locator('[data-testid^="point-restore-clearsweep:"]').first();
    await expect(app.locator('[data-testid^="point-kind-clearsweep:"]').first()).toHaveText('Configuration backup');
    await fits(app);
    await restoreBtn.click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('Restore this backup?');
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('restore-note')).toContainText('Restored 4 items');
    expect(readFileSync(f.launcher, 'utf8')).toBe(BROKEN);
    expect(readFileSync(f.mimeapps, 'utf8')).toBe(MIME);

    // delete it: a backup, unlike a system point, has no "newest" protection
    await app.locator('[data-testid^="point-delete-clearsweep:"]').first().click();
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('restore-backups')).toHaveCount(0);
  });

  test('explains what to install when no tool is present', async ({ page, bare }) => {
    const app = await open(page, bare);
    await openTool(app, 'restore');
    await expect(app.getByTestId('restore-unsupported')).toBeVisible();
    await expect(app.getByTestId('restore-hint')).toContainText('Timeshift');
    await expect(app.getByTestId('restore-hint')).toContainText('Snapper');
    await expect(app.getByTestId('restore-create')).toHaveCount(0);
    await fits(app);
  });
});

test('Config Issues page keeps working when a symlink target is missing', async ({ app, machine }) => {
  // a dangling link whose target lives on an unmounted drive is left alone
  mkdirSync(path.join(machine.home, '.local/bin'), { recursive: true });
  symlinkSync('/mnt/nas/tools/x', path.join(machine.home, '.local/bin/nas'));
  await openTool(app, 'registry');
  await app.getByTestId('registry-scan').click();
  await expect(app.getByTestId('registry-empty')).toBeVisible();
});
