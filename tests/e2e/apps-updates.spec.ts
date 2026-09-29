/**
 * Uninstall, Software Updater and Driver Updater against fake `dpkg-query` / `apt` / `apt-get` /
 * `fwupdmgr` executables (the server's PATH contains nothing else). Runs at 380 and 320 px.
 */
import { test as base, expect, type Page } from '@playwright/test';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { noHorizontalScroll } from './fixtures';
import { FWUPD_JSON, startFakeMachine, type FakeMachine } from './helpers/fakeSystem';

base.skip(process.platform !== 'linux', 'uses shell-script fakes and the Linux layout');

const test = base.extend<{ machine: FakeMachine; app: Page }>({
  machine: async ({}, use) => {
    const m = await startFakeMachine();
    try {
      await use(m);
    } finally {
      await m.stop();
    }
  },
  app: async ({ page, machine }, use) => {
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
    await use(page);
    expect(errors, 'no console errors / page errors').toEqual([]);
  },
});

async function openTool(app: Page, id: string, pageId: string): Promise<void> {
  await app.getByTestId('tab-tools').click();
  await app.getByTestId(`tile-${id}`).click();
  await expect(app.getByTestId(pageId)).toBeVisible();
}

async function fits(app: Page): Promise<void> {
  const m = await noHorizontalScroll(app);
  expect(m.scrollWidth, 'no horizontal scroll').toBeLessThanOrEqual(m.innerWidth);
}

test.describe('Uninstall', () => {
  test('lists packages, uninstalls one, shows and removes its leftovers', async ({ app, machine }) => {
    // Leftovers of clearsweep-demo, plus an unrelated folder that must survive.
    const mine = path.join(machine.configDir, 'clearsweep-demo');
    const other = path.join(machine.configDir, 'unrelated-app');
    for (const d of [mine, other]) mkdirSync(d, { recursive: true });
    writeFileSync(path.join(mine, 'settings.json'), '{"a":1}');
    writeFileSync(path.join(other, 'keep.txt'), 'keep');

    await openTool(app, 'uninstall', 'page-uninstall');
    await expect(app.getByTestId('app-row-dpkg:clearsweep-demo')).toBeVisible();
    await expect(app.getByTestId('app-row-dpkg:htop')).toBeVisible();
    // essential packages are hidden until asked for
    await expect(app.getByTestId('app-row-dpkg:bash')).toHaveCount(0);
    await expect(app.getByTestId('app-row-dpkg:clearsweep-demo')).toContainText('1.0.0 - Demo Maker');
    await expect(app.getByTestId('app-row-dpkg:clearsweep-demo')).toContainText('120 KB');
    await fits(app);

    await app.getByTestId('uninstall-search').fill('demo');
    await expect(app.getByTestId('uninstall-list')).toContainText('Installed programs (1)');
    await app.getByTestId('app-row-dpkg:clearsweep-demo').click();
    await expect(app.getByTestId('app-sheet')).toBeVisible();
    await expect(app.getByTestId('app-action-repair')).toHaveCount(0);
    await expect(app.getByTestId('app-action-rename')).toHaveCount(0);
    await fits(app);
    await app.getByTestId('app-action-uninstall').click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('Uninstall clearsweep-demo?');
    await fits(app);
    expect(machine.log().filter((l) => l.startsWith('apt-get remove'))).toEqual([]);
    await app.getByTestId('confirm-sheet-confirm').click();

    await expect(app.getByTestId('uninstall-note')).toContainText('clearsweep-demo');
    expect(machine.readState('dpkg.txt')).not.toContain('clearsweep-demo');
    expect(machine.log()).toContain('apt-get remove -y clearsweep-demo');
    await expect(app.getByTestId('app-row-dpkg:clearsweep-demo')).toHaveCount(0);

    const card = app.getByTestId('leftovers-card');
    await expect(card).toBeVisible();
    await expect(card).toContainText(mine);
    await expect(card).not.toContainText(other);
    await fits(app);
    await app.getByTestId('btn-remove-leftovers').click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('Remove leftovers?');
    expect(existsSync(mine)).toBe(true); // nothing is deleted before confirming
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('uninstall-note')).toContainText('Removed 1 of 1 leftover items');
    expect(existsSync(mine)).toBe(false);
    expect(existsSync(path.join(other, 'keep.txt'))).toBe(true);
    await expect(app.getByTestId('leftovers-card')).toHaveCount(0);
  });

  test('a package that drags others along needs a second confirmation', async ({ app, machine }) => {
    await openTool(app, 'uninstall', 'page-uninstall');
    await app.getByTestId('app-row-dpkg:htop').click();
    await app.getByTestId('app-action-uninstall').click();
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('Also remove 1 other package?');
    await expect(app.getByTestId('confirm-sheet')).toContainText('htop-plugins');
    await fits(app);
    // nothing has been removed yet
    expect(machine.readState('dpkg.txt')).toContain('htop');
    expect(machine.log().filter((l) => l.startsWith('apt-get remove'))).toEqual([]);
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('uninstall-note')).toContainText('htop');
    expect(machine.readState('dpkg.txt')).not.toContain('htop');
  });

  test('system components are hidden, then shown with a strong warning', async ({ app, machine }) => {
    await openTool(app, 'uninstall', 'page-uninstall');
    await app.getByTestId('uninstall-show-system').check();
    await app.getByTestId('app-row-dpkg:bash').click();
    await expect(app.getByTestId('app-sheet-system')).toBeVisible();
    await app.getByTestId('app-action-uninstall').click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('system component');
    await fits(app);
    await app.getByRole('button', { name: 'Cancel' }).click();
    await expect(app.getByTestId('confirm-sheet')).toHaveCount(0);
    expect(machine.log().filter((l) => l.startsWith('apt-get remove'))).toEqual([]);
    expect(machine.readState('dpkg.txt')).toContain('bash');
  });

  test('the server refuses to remove an essential package without force, whatever the client sends', async ({ machine }) => {
    const res = await fetch(`${machine.server.origin}/api/call`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Sweep-Token': machine.server.token },
      body: JSON.stringify({ callId: 'c1', method: 'uninstall.run', params: { id: 'dpkg:bash' } }),
    });
    const last = JSON.parse((await res.text()).trim().split('\n').pop()!) as { type: string; error?: { code: string } };
    expect(last.type).toBe('error');
    expect(last.error?.code).toBe('PermissionDenied');
    expect(machine.readState('dpkg.txt')).toContain('bash');
    expect(machine.log().filter((l) => l.startsWith('apt-get remove'))).toEqual([]);
  });
});

test.describe('Software Updater', () => {
  test('lists updates, updates a selection, ignores another and skips it in Update all', async ({ app, machine }) => {
    await openTool(app, 'updater', 'page-updater');
    await expect(app.getByTestId('updater-list')).toBeVisible();
    await expect(app.getByTestId('updater-summary')).toContainText('2 updates available');
    const ff = app.getByTestId('update-apt:firefox');
    await expect(ff.getByTestId('update-versions')).toContainText('125.0.3 → 126.0+build2');
    await expect(ff.getByTestId('update-security')).toBeVisible();
    await expect(ff.getByTestId('update-source')).toHaveText('apt');
    await expect(app.getByTestId('update-apt:vim').getByTestId('update-security')).toHaveCount(0);
    await fits(app);

    // update just firefox
    await app.getByTestId('update-select-apt:firefox').check();
    await app.getByTestId('btn-update-selected').click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('Update 1 selected program?');
    await fits(app);
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('updater-report')).toContainText('1 succeeded');
    expect(machine.log()).toContain('apt-get install --only-upgrade -y firefox');
    await expect(app.getByTestId('update-apt:firefox')).toHaveCount(0);
    await expect(app.getByTestId('update-apt:vim')).toBeVisible();

    // ignore vim: it can no longer be selected and Update all has nothing to do
    await app.getByTestId('update-menu-apt:vim').click();
    await app.getByTestId('update-toggle-ignore').click();
    await expect(app.getByTestId('update-select-apt:vim')).toBeDisabled();
    await expect(app.getByTestId('update-apt:vim').getByTestId('update-ignored')).toBeVisible();
    await expect(app.getByTestId('btn-update-all')).toBeDisabled();
    await fits(app);
    // the choice survives a reload of the list
    await app.getByTestId('btn-refresh').click();
    await expect(app.getByTestId('update-apt:vim').getByTestId('update-ignored')).toBeVisible();
    expect(machine.log().filter((l) => l.includes('install') && l.includes('vim'))).toEqual([]);
    expect(machine.log()).toContain('apt-get update'); // Refresh updates the package index first
  });

  test('Update all updates everything that is not ignored', async ({ app, machine }) => {
    await openTool(app, 'updater', 'page-updater');
    await app.getByTestId('btn-update-all').click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('Update all 2 programs?');
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('updater-report')).toContainText('2 succeeded');
    expect(machine.log()).toContain('apt-get install --only-upgrade -y firefox vim');
    await expect(app.getByTestId('updater-summary')).toContainText('Everything is up to date.');
    await fits(app);
  });

  test('the server only updates ids that are really available', async ({ machine }) => {
    const res = await fetch(`${machine.server.origin}/api/call`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Sweep-Token': machine.server.token },
      body: JSON.stringify({
        callId: 'c2',
        method: 'software_updater.update',
        params: { ids: ['apt:vim', 'apt:evil; touch /tmp/pwned', 'apt:--purge'] },
      }),
    });
    const last = JSON.parse((await res.text()).trim().split('\n').pop()!) as {
      value: { results: { id: string; ok: boolean }[] };
    };
    expect(last.value.results.filter((r) => r.ok).map((r) => r.id)).toEqual(['apt:vim']);
    expect(machine.log().filter((l) => l.startsWith('apt-get install'))).toEqual(['apt-get install --only-upgrade -y vim']);
  });
});

test.describe('Driver Updater', () => {
  test('scan, install a firmware update, restart notice', async ({ app, machine }) => {
    machine.writeState('fwupd.json', FWUPD_JSON);
    await openTool(app, 'drivers', 'page-drivers');
    // nothing happens until Scan is pressed
    expect(machine.log().filter((l) => l.startsWith('fwupdmgr'))).toEqual([]);
    await expect(app.getByTestId('btn-backup')).toHaveCount(0); // Windows only
    await app.getByTestId('btn-scan').click();
    const row = app.getByTestId('driver-fwupd:3fc0a2fa8d0ff8ec5b4d0d4dbbc91ba6a3d1d1d1');
    await expect(row).toBeVisible();
    await expect(row.getByTestId('driver-name')).toHaveText('System Firmware');
    await expect(row.getByTestId('driver-versions')).toContainText('Dell - 1.20.0 → 1.22.0');
    await expect(row.getByTestId('driver-reboot')).toBeVisible();
    await fits(app);
    expect(machine.log()).toContain('fwupdmgr get-updates --json');

    await row.getByRole('checkbox').check();
    await app.getByTestId('btn-update-selected').click();
    await expect(app.getByTestId('confirm-sheet')).toContainText('Install 1 driver update?');
    await fits(app);
    await app.getByTestId('confirm-sheet-confirm').click();
    await expect(app.getByTestId('reboot-notice')).toBeVisible();
    await expect(app.getByTestId('drivers-report')).toContainText('1 succeeded');
    expect(machine.log()).toContain(
      'fwupdmgr update 3fc0a2fa8d0ff8ec5b4d0d4dbbc91ba6a3d1d1d1 -y --no-reboot-check',
    );
    // rescan after the update: exit code 2 from fwupdmgr is "nothing to do", not an error
    await expect(app.getByTestId('drivers-empty')).toBeVisible();
    await expect(app.getByTestId('error-banner')).toHaveCount(0);
    await fits(app);
  });

  test('"nothing to update" (fwupdmgr exit 2) is an empty result, not an error', async ({ app }) => {
    await openTool(app, 'drivers', 'page-drivers');
    await app.getByTestId('btn-scan').click();
    await expect(app.getByTestId('drivers-empty')).toContainText('No driver updates found');
    await expect(app.getByTestId('error-banner')).toHaveCount(0);
  });
});
