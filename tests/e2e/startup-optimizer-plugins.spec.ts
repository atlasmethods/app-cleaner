/**
 * Startup manager, Performance (sleep mode) and Browser Plugins against a throwaway machine:
 * real files under a temp HOME, fake `systemctl` / `crontab` executables, a scripted process
 * list. Runs at 380 and 320 px.
 */
import { test as base, expect, type Page } from '@playwright/test';
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { noHorizontalScroll } from './fixtures';
import { bootMachine, type MachineOptions, type StartupMachine } from './helpers/startupMachine';

base.skip(process.platform !== 'linux', 'uses shell-script fakes and the Linux layout');

interface Fixtures {
  boot: (setup: (m: StartupMachine) => void, opts?: MachineOptions) => Promise<StartupMachine>;
  errors: string[];
}

const test = base.extend<Fixtures>({
  errors: async ({ page }, use) => {
    const errors: string[] = [];
    page.on('console', (msg) => {
      // Failed API calls are expected in the error-path tests and logged by the browser as resource errors.
      if (msg.type() === 'error' && !/status of (4|5)\d\d/.test(msg.text())) errors.push(`console.error: ${msg.text()}`);
    });
    page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
    await use(errors);
    expect(errors, 'no console errors / page errors').toEqual([]);
  },
  boot: async ({ errors }, use) => {
    void errors;
    const machines: StartupMachine[] = [];
    await use(async (setup, opts) => {
      const m = await bootMachine(setup, opts);
      machines.push(m);
      return m;
    });
    for (const m of machines) await m.stop();
  },
});

async function open(page: Page, m: StartupMachine, target: { tab: 'performance' } | { tool: string; page: string }): Promise<void> {
  await page.goto(m.server.url);
  await expect(page.getByTestId('tabbar')).toBeVisible();
  if ('tab' in target) {
    await page.getByTestId(`tab-${target.tab}`).click();
    await expect(page.getByTestId(`page-${target.tab}`)).toBeVisible();
  } else {
    await page.getByTestId('tab-tools').click();
    await page.getByTestId(`tile-${target.tool}`).click();
    await expect(page.getByTestId(target.page)).toBeVisible();
  }
}

async function fits(page: Page): Promise<void> {
  const m = await noHorizontalScroll(page);
  expect(m.scrollWidth, 'no horizontal scroll').toBeLessThanOrEqual(m.innerWidth);
}

function write(p: string, text: string): void {
  mkdirSync(path.dirname(p), { recursive: true });
  writeFileSync(p, text);
}

const SLACK = '[Desktop Entry]\nType=Application\nName=Slack\nExec=/opt/Slack/slack -u %U\n';
const BLUEMAN = '[Desktop Entry]\nType=Application\nName=Blueman Applet\nExec=blueman-applet\n';
const POLKIT = '[Desktop Entry]\nName=PolicyKit Agent\nExec=/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1\n';
const CRON = '# my jobs\n*/5 * * * * /usr/bin/true\n@reboot /home/u/bin/sync.sh --quiet\n@daily /bin/echo hi\n';

// ---------------------------------------------------------------- Startup

function startupSetup(m: StartupMachine): void {
  write(path.join(m.autostartDir, 'slack.desktop'), SLACK);
  write(m.sys('/etc/xdg/autostart/blueman.desktop'), BLUEMAN);
  write(m.sys('/etc/xdg/autostart/polkit-gnome-authentication-agent-1.desktop'), POLKIT);
  m.writeState('user-units.txt', 'syncthing.service enabled\npipewire.service enabled\n');
  m.writeState('system-units.txt', 'ssh.service enabled\ndbus.service enabled\nmyapp.service enabled\n');
  m.writeState('crontab', CRON);
}

test.describe('Startup', () => {
  test('lists items, filters, and shows critical items locked behind a toggle', async ({ page, boot }) => {
    const m = await boot(startupSetup);
    await open(page, m, { tool: 'startup', page: 'page-startup' });
    await expect(page.getByTestId('startup-list')).toBeVisible();
    await expect(page.getByTestId('startup-row-xdg:user:slack.desktop')).toContainText('Slack');
    await expect(page.getByTestId('startup-row-xdg:system:blueman.desktop')).toBeVisible();
    await expect(page.getByTestId('startup-row-systemd:user:syncthing.service')).toBeVisible();
    await expect(page.getByTestId('startup-row-systemd:system:myapp.service')).toBeVisible();
    // critical ones hidden by default
    await expect(page.getByTestId('startup-row-systemd:system:dbus.service')).toHaveCount(0);
    await expect(page.getByTestId('startup-row-xdg:system:polkit-gnome-authentication-agent-1.desktop')).toHaveCount(0);
    await expect(page.getByTestId('chip-autostart')).toBeVisible();
    await expect(page.getByTestId('chip-service')).toBeVisible();
    await expect(page.getByTestId('chip-cron')).toBeVisible();
    await expect(page.getByTestId('chip-context_menu')).toHaveCount(0);
    await fits(page);

    await page.getByTestId('chip-cron').click();
    await expect(page.getByTestId('startup-list')).toContainText('Startup items (1)');
    await page.getByTestId('chip-all').click();
    await page.getByTestId('startup-search').fill('syncthing');
    await expect(page.getByTestId('startup-list')).toContainText('Startup items (1)');
    await page.getByTestId('startup-search').fill('');

    await page.getByTestId('startup-show-system').check();
    const dbus = page.getByTestId('startup-row-systemd:system:dbus.service');
    await expect(dbus).toBeVisible();
    await expect(dbus.getByLabel('System item')).toBeVisible();
    await expect(page.getByTestId('startup-toggle-systemd:system:dbus.service')).toBeDisabled();
    await fits(page);
  });

  test('toggling an autostart entry edits the file and restores it byte for byte', async ({ page, boot }) => {
    const m = await boot(startupSetup);
    await open(page, m, { tool: 'startup', page: 'page-startup' });
    const file = path.join(m.autostartDir, 'slack.desktop');
    const sw = page.getByTestId('startup-toggle-xdg:user:slack.desktop');
    await expect(sw).toHaveAttribute('aria-checked', 'true');
    await sw.click();
    await expect(sw).toHaveAttribute('aria-checked', 'false');
    await expect(page.getByTestId('startup-note')).toContainText('Slack is now off');
    expect(readFileSync(file, 'utf8')).toBe(`${SLACK}Hidden=true\n`);
    await sw.click();
    await expect(sw).toHaveAttribute('aria-checked', 'true');
    expect(readFileSync(file, 'utf8')).toBe(SLACK);
    await fits(page);
  });

  test('a system autostart entry is disabled by a user override and re-enabled by removing it', async ({ page, boot }) => {
    const m = await boot(startupSetup);
    await open(page, m, { tool: 'startup', page: 'page-startup' });
    const sw = page.getByTestId('startup-toggle-xdg:system:blueman.desktop');
    await sw.click();
    // machine-wide: confirmed first
    await expect(page.getByTestId('confirm-sheet')).toContainText('Turn off Blueman Applet?');
    await fits(page);
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(sw).toHaveAttribute('aria-checked', 'false');
    const override = path.join(m.autostartDir, 'blueman.desktop');
    expect(readFileSync(override, 'utf8')).toContain('Hidden=true');
    expect(readFileSync(m.sys('/etc/xdg/autostart/blueman.desktop'), 'utf8')).toBe(BLUEMAN);
    await sw.click();
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(sw).toHaveAttribute('aria-checked', 'true');
    expect(existsSync(override)).toBe(false);
  });

  test('systemd units toggle through systemctl, with a warning confirm for ssh', async ({ page, boot }) => {
    const m = await boot(startupSetup);
    await open(page, m, { tool: 'startup', page: 'page-startup' });
    const user = page.getByTestId('startup-toggle-systemd:user:syncthing.service');
    await user.click();
    await expect(user).toHaveAttribute('aria-checked', 'false');
    expect(m.readState('user-units.txt')).toContain('syncthing.service disabled');
    expect(m.log()).toContain('systemctl --user disable syncthing.service');

    const ssh = page.getByTestId('startup-toggle-systemd:system:ssh.service');
    await expect(page.getByTestId('startup-row-systemd:system:ssh.service').getByTestId('startup-warning')).toContainText('SSH');
    await ssh.click();
    await expect(page.getByTestId('confirm-sheet')).toContainText('Disabling SSH');
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(ssh).toHaveAttribute('aria-checked', 'false');
    expect(m.readState('system-units.txt')).toContain('ssh.service disabled');
  });

  test('a cron @reboot line is commented out and restored, leaving other lines alone', async ({ page, boot }) => {
    const m = await boot(startupSetup);
    await open(page, m, { tool: 'startup', page: 'page-startup' });
    await page.getByTestId('chip-cron').click();
    const row = page.locator('[data-testid^="startup-row-cron:"]');
    await expect(row).toHaveCount(1);
    const sw = row.locator('[role="switch"]');
    await sw.click();
    await expect(sw).toHaveAttribute('aria-checked', 'false');
    expect(m.readState('crontab')).toBe(CRON.replace('@reboot /home/u', '# clearsweep-disabled: @reboot /home/u'));
    await sw.click();
    await expect(sw).toHaveAttribute('aria-checked', 'true');
    expect(m.readState('crontab')).toBe(CRON);
  });

  test('deleting asks first, keeps a backup and can be restored', async ({ page, boot }) => {
    const m = await boot(startupSetup);
    await open(page, m, { tool: 'startup', page: 'page-startup' });
    const file = path.join(m.autostartDir, 'slack.desktop');
    await page.getByTestId('startup-menu-xdg:user:slack.desktop').click();
    await expect(page.getByTestId('startup-sheet')).toBeVisible();
    await expect(page.getByTestId('startup-sheet-command')).toContainText('/opt/Slack/slack');
    await fits(page);
    await page.getByTestId('startup-action-delete').click();
    await expect(page.getByTestId('confirm-sheet')).toContainText('A backup is saved first');
    await fits(page);
    expect(existsSync(file)).toBe(true);
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(page.getByTestId('startup-row-xdg:user:slack.desktop')).toHaveCount(0);
    await expect(page.getByTestId('startup-note')).toContainText('Backup saved as startup-');
    expect(existsSync(file)).toBe(false);
    const backups = readdirSync(path.join(m.dataDir, 'backups')).filter((d) => d.startsWith('startup-'));
    expect(backups).toHaveLength(1);
    const manifest = JSON.parse(readFileSync(path.join(m.dataDir, 'backups', backups[0]!, 'manifest.json'), 'utf8'));
    expect(manifest.kind).toBe('startup');
    // restore over the API, then the item is back
    const res = await fetch(`${m.server.origin}/api/call`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Sweep-Token': m.server.token },
      body: JSON.stringify({ callId: 'r1', method: 'startup.restore_backup', params: { id: backups[0] } }),
    });
    expect(await res.text()).toContain('"restored":1');
    expect(readFileSync(file, 'utf8')).toBe(SLACK);
  });
});

// ---------------------------------------------------------------- Performance

const SLACK_PROCS = 'slack|/opt/Slack/slack|300|3,slack|/opt/Slack/slack|120|1,bash|/usr/bin/bash|5|0,gnome-shell|/usr/bin/gnome-shell|400|2';

test.describe('Performance', () => {
  test('summary, sleep with confirm, sleeping section and wake', async ({ page, boot }) => {
    const m = await boot(
      (mm) => {
        write(path.join(mm.autostartDir, 'slack.desktop'), SLACK);
        write(path.join(mm.autostartDir, 'slack-tray.desktop'), '[Desktop Entry]\nName=Slack tray\nExec=/opt/Slack/slack --tray\nHidden=true\n');
      },
      { processes: SLACK_PROCS },
    );
    await open(page, m, { tab: 'performance' });
    await expect(page.getByTestId('perf-list')).toBeVisible();
    await expect(page.getByTestId('perf-summary-count')).toHaveText('1');
    await expect(page.getByTestId('perf-summary-memory')).toHaveText('420 MB');
    await expect(page.getByTestId('perf-row-slack')).toContainText('420 MB');
    await expect(page.getByTestId('perf-explain')).toContainText('Wake puts its startup items back');
    // session / shell processes are not listed
    await expect(page.getByTestId('perf-name')).toHaveText(['Slack']);
    await fits(page);

    await page.getByTestId('perf-sleep-slack').click();
    await expect(page.getByTestId('confirm-sheet')).toContainText('Put Slack to sleep?');
    await fits(page);
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(page.getByTestId('perf-sleeping-slack')).toBeVisible();
    // The e2e server sees a fixed process list and cannot signal anything: it says so.
    await expect(page.getByTestId('perf-note')).toContainText('still running');
    const main = readFileSync(path.join(m.autostartDir, 'slack.desktop'), 'utf8');
    expect(main).toBe(`${SLACK}Hidden=true\n`);
    const state = JSON.parse(readFileSync(path.join(m.dataDir, 'optimizer.json'), 'utf8'));
    expect(state.sleeping.slack.disabled.map((d: { id: string }) => d.id)).toEqual(['xdg:user:slack.desktop']);
    expect(state.sleeping.slack.alreadyDisabled).toEqual(['xdg:user:slack-tray.desktop']);
    await fits(page);

    await page.getByTestId('perf-wake-slack').click();
    await expect(page.getByTestId('perf-note')).toContainText('1 app woken');
    await expect(page.getByTestId('perf-sleeping')).toHaveCount(0);
    expect(readFileSync(path.join(m.autostartDir, 'slack.desktop'), 'utf8')).toBe(SLACK);
    // the entry the user had disabled before stays disabled
    expect(readFileSync(path.join(m.autostartDir, 'slack-tray.desktop'), 'utf8')).toContain('Hidden=true');
  });

  test('Sleep all is confirmed and skips protected apps', async ({ page, boot }) => {
    const m = await boot(
      (mm) => {
        write(path.join(mm.autostartDir, 'slack.desktop'), SLACK);
        write(path.join(mm.autostartDir, 'malwarebytes.desktop'), '[Desktop Entry]\nName=Malwarebytes\nExec=/opt/malwarebytes/mbam\n');
      },
      { processes: 'slack|/opt/Slack/slack|100|1,mbam|/opt/malwarebytes/mbam|80|1' },
    );
    await open(page, m, { tab: 'performance' });
    await expect(page.getByTestId('perf-row-mbam')).toContainText('Protected');
    await expect(page.getByTestId('perf-sleep-mbam')).toBeDisabled();
    await expect(page.getByTestId('perf-sleep-all')).toContainText('Sleep all (1)');
    await page.getByTestId('perf-sleep-all').click();
    await expect(page.getByTestId('confirm-sheet')).toContainText('Slack');
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(page.getByTestId('perf-sleeping-slack')).toBeVisible();
    expect(readFileSync(path.join(m.autostartDir, 'malwarebytes.desktop'), 'utf8')).not.toContain('Hidden');
  });

  test('enforce re-disables a re-created entry (over the API) without touching processes', async ({ page, boot }) => {
    const m = await boot((mm) => write(path.join(mm.autostartDir, 'slack.desktop'), SLACK), { processes: SLACK_PROCS });
    await open(page, m, { tab: 'performance' });
    await page.getByTestId('perf-sleep-slack').click();
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(page.getByTestId('perf-sleeping-slack')).toBeVisible();
    write(path.join(m.autostartDir, 'slack.desktop'), SLACK); // the app re-registered itself
    const res = await fetch(`${m.server.origin}/api/call`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Sweep-Token': m.server.token },
      body: JSON.stringify({ callId: 'e1', method: 'optimizer.enforce', params: {} }),
    });
    expect(await res.text()).toContain('re-enabled');
    expect(readFileSync(path.join(m.autostartDir, 'slack.desktop'), 'utf8')).toContain('Hidden=true');
  });
});

// ---------------------------------------------------------------- Browser plugins

const EXT_A = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const EXT_B = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';

function chromeSetup(m: StartupMachine): { profile: string; prefs: string } {
  const profile = path.join(m.configDir, 'google-chrome', 'Default');
  write(path.join(profile, 'Extensions', EXT_A, '1.0_0', 'manifest.json'), '{"name":"Ad Blocker","version":"1.0","description":"Blocks ads"}');
  write(path.join(profile, 'Extensions', EXT_B, '2.0_0', 'manifest.json'), '{"name":"Protected Ext","version":"2.0"}');
  const prefs = path.join(profile, 'Preferences');
  write(
    prefs,
    JSON.stringify({
      profile: { name: 'Work' },
      other: { keep: [1, 2, 3] },
      extensions: { settings: { [EXT_A]: { location: 1, path: `${EXT_A}/1.0_0`, state: 1 } } },
    }),
  );
  write(
    path.join(profile, 'Secure Preferences'),
    JSON.stringify({ extensions: { settings: { [EXT_B]: { location: 1, path: `${EXT_B}/2.0_0`, state: 1 } } } }),
  );
  return { profile, prefs };
}

test.describe('Browser Plugins', () => {
  test('lists grouped add-ons, toggles one, explains the protected one', async ({ page, boot }) => {
    let prefs = '';
    const m = await boot((mm) => {
      prefs = chromeSetup(mm).prefs;
    });
    await open(page, m, { tool: 'plugins', page: 'page-plugins' });
    await expect(page.getByTestId('plugins-browser-chrome')).toContainText('Google Chrome');
    await expect(page.getByTestId('plugins-browser-chrome')).toContainText('Work (Default)');
    const a = page.getByTestId(`plugin-row-chrome:Default:${EXT_A}`);
    await expect(a).toContainText('Ad Blocker');
    await expect(a).toContainText('Version 1.0 - Blocks ads');
    await expect(a.getByTestId('plugin-type')).toHaveText('Extension');
    const b = page.getByTestId(`plugin-row-chrome:Default:${EXT_B}`);
    await expect(b.getByTestId('plugin-note')).toContainText('protects extension settings');
    await expect(page.getByTestId(`plugin-toggle-chrome:Default:${EXT_B}`)).toBeDisabled();
    await fits(page);

    const sw = page.getByTestId(`plugin-toggle-chrome:Default:${EXT_A}`);
    await sw.click();
    await expect(sw).toHaveAttribute('aria-checked', 'false');
    const saved = JSON.parse(readFileSync(prefs, 'utf8'));
    expect(saved.other).toEqual({ keep: [1, 2, 3] });
    expect(saved.extensions.settings[EXT_A].disable_reasons).toBe(1);
    expect(saved.extensions.settings[EXT_A].state).toBe(0);
    expect(readdirSync(path.join(m.dataDir, 'backups')).some((d) => d.startsWith('plugins-'))).toBe(true);
    await sw.click();
    await expect(sw).toHaveAttribute('aria-checked', 'true');
    await fits(page);
  });

  test('removing asks first and deletes the extension folder', async ({ page, boot }) => {
    let profile = '';
    const m = await boot((mm) => {
      profile = chromeSetup(mm).profile;
    });
    await open(page, m, { tool: 'plugins', page: 'page-plugins' });
    await page.getByTestId(`plugin-remove-chrome:Default:${EXT_A}`).click();
    await expect(page.getByTestId('confirm-sheet')).toContainText('Remove Ad Blocker?');
    await fits(page);
    expect(existsSync(path.join(profile, 'Extensions', EXT_A))).toBe(true);
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(page.getByTestId(`plugin-row-chrome:Default:${EXT_A}`)).toHaveCount(0);
    await expect(page.getByTestId('plugins-note')).toContainText('Backup saved as plugins-');
    expect(existsSync(path.join(profile, 'Extensions', EXT_A))).toBe(false);
    const prefs = JSON.parse(readFileSync(path.join(profile, 'Preferences'), 'utf8'));
    expect(prefs.extensions.settings[EXT_A]).toBeUndefined();
  });

  test('a running browser produces a friendly "close it first" message', async ({ page, boot }) => {
    const m = await boot((mm) => void chromeSetup(mm), { processes: 'chrome|/opt/google/chrome/chrome|300|1' });
    await open(page, m, { tool: 'plugins', page: 'page-plugins' });
    await expect(page.getByTestId('plugins-running-chrome')).toContainText('close it to make changes');
    await page.getByTestId(`plugin-toggle-chrome:Default:${EXT_A}`).click();
    await expect(page.getByTestId('plugins-close-notice')).toContainText('Close Google Chrome first');
    await expect(page.getByTestId(`plugin-toggle-chrome:Default:${EXT_A}`)).toHaveAttribute('aria-checked', 'true');
    await fits(page);
  });

  test('shows Firefox add-ons and an empty state without profiles', async ({ page, boot }) => {
    const m = await boot((mm) => {
      const profile = path.join(mm.home, '.mozilla/firefox/abcd1234.default-release');
      write(path.join(mm.home, '.mozilla/firefox/profiles.ini'), '[Profile0]\nName=default\nIsRelative=1\nPath=abcd1234.default-release\nDefault=1\n');
      write(path.join(profile, 'prefs.js'), '');
      write(
        path.join(profile, 'extensions.json'),
        JSON.stringify({
          addons: [
            { id: 'ubo@example.net', type: 'extension', active: true, userDisabled: false, location: 'app-profile', version: '1.60', visible: true, defaultLocale: { name: 'uBlock Origin' } },
            { id: 'builtin@mozilla.org', type: 'extension', active: true, userDisabled: false, location: 'app-builtin', version: '1', visible: true, defaultLocale: { name: 'Built in' } },
          ],
        }),
      );
      write(path.join(profile, 'extensions', 'ubo@example.net.xpi'), 'zip');
    });
    await open(page, m, { tool: 'plugins', page: 'page-plugins' });
    await expect(page.getByTestId('plugins-browser-firefox')).toContainText('uBlock Origin');
    await expect(page.getByTestId('plugins-browser-firefox')).not.toContainText('Built in');
    await page.getByTestId('plugins-search').fill('zzzz');
    await expect(page.getByTestId('plugins-empty')).toContainText('Nothing matches');
    await fits(page);
  });
});

// ---------------------------------------------------------------- Undo and System Restore

const UBO = 'ubo@example.net';

function firefoxSetup(m: StartupMachine): { profile: string } {
  const profile = path.join(m.home, '.mozilla/firefox/abcd1234.default-release');
  write(path.join(m.home, '.mozilla/firefox/profiles.ini'), '[Profile0]\nName=default\nIsRelative=1\nPath=abcd1234.default-release\nDefault=1\n');
  write(path.join(profile, 'prefs.js'), '');
  write(
    path.join(profile, 'extensions.json'),
    JSON.stringify({
      schemaVersion: 36,
      addons: [{ id: UBO, type: 'extension', active: true, userDisabled: false, location: 'app-profile', version: '1.60', visible: true, defaultLocale: { name: 'uBlock Origin' } }],
    }),
  );
  write(path.join(profile, 'extensions', `${UBO}.xpi`), 'zip-bytes');
  return { profile };
}

test.describe('Restoring removed items', () => {
  test('deleting a startup item offers Undo, which puts the file back', async ({ page, boot }) => {
    const m = await boot(startupSetup);
    await open(page, m, { tool: 'startup', page: 'page-startup' });
    const file = path.join(m.autostartDir, 'slack.desktop');
    const row = page.getByTestId('startup-row-xdg:user:slack.desktop');
    await page.getByTestId('startup-menu-xdg:user:slack.desktop').click();
    await page.getByTestId('startup-action-delete').click();
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(row).toHaveCount(0);
    expect(existsSync(file)).toBe(false);
    await expect(page.getByTestId('startup-undo')).toBeVisible();
    await fits(page);
    await page.getByTestId('startup-undo').click();
    await expect(row).toBeVisible();
    await expect(page.getByTestId('startup-note')).toContainText('Slack is back.');
    await expect(page.getByTestId('startup-undo')).toHaveCount(0);
    expect(readFileSync(file, 'utf8')).toBe(SLACK);
    await fits(page);
  });

  test('a deleted startup item is listed in System Restore and restored from there', async ({ page, boot }) => {
    const m = await boot(startupSetup);
    await open(page, m, { tool: 'startup', page: 'page-startup' });
    const file = path.join(m.autostartDir, 'slack.desktop');
    await page.getByTestId('startup-menu-xdg:user:slack.desktop').click();
    await page.getByTestId('startup-action-delete').click();
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(page.getByTestId('startup-undo')).toBeVisible();
    expect(existsSync(file)).toBe(false);

    await open(page, m, { tool: 'restore', page: 'page-restore' });
    const card = page.getByTestId('restore-backups');
    await expect(card).toContainText('Removed startup item: Slack');
    await expect(card).toContainText('Startup item');
    await fits(page);
    await card.locator('[data-testid^="point-restore-"]').click();
    await expect(page.getByTestId('confirm-sheet')).toContainText('put back as it was');
    await fits(page);
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(page.getByTestId('restore-note')).toContainText('Restored startup item Slack.');
    expect(readFileSync(file, 'utf8')).toBe(SLACK);
    await fits(page);
  });

  test('removing a Chromium add-on offers Undo', async ({ page, boot }) => {
    let profile = '';
    let prefsBefore = '';
    const m = await boot((mm) => {
      const c = chromeSetup(mm);
      profile = c.profile;
      prefsBefore = readFileSync(c.prefs, 'utf8');
    });
    await open(page, m, { tool: 'plugins', page: 'page-plugins' });
    const row = page.getByTestId(`plugin-row-chrome:Default:${EXT_A}`);
    await page.getByTestId(`plugin-remove-chrome:Default:${EXT_A}`).click();
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(row).toHaveCount(0);
    expect(existsSync(path.join(profile, 'Extensions', EXT_A))).toBe(false);
    await fits(page);
    await page.getByTestId('plugins-undo').click();
    await expect(row).toBeVisible();
    await expect(page.getByTestId('plugins-note')).toContainText('Ad Blocker is back.');
    expect(readFileSync(path.join(profile, 'Extensions', EXT_A, '1.0_0', 'manifest.json'), 'utf8')).toContain('Ad Blocker');
    expect(JSON.parse(readFileSync(path.join(profile, 'Preferences'), 'utf8'))).toEqual(JSON.parse(prefsBefore));
    await fits(page);
  });

  test('a removed Firefox add-on is restored from System Restore', async ({ page, boot }) => {
    let profile = '';
    const m = await boot((mm) => {
      profile = firefoxSetup(mm).profile;
    });
    await open(page, m, { tool: 'plugins', page: 'page-plugins' });
    const id = `firefox:abcd1234.default-release:${UBO}`;
    await page.getByTestId(`plugin-remove-${id}`).click();
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(page.getByTestId('plugins-note')).toContainText('Backup saved as plugins-');
    const xpi = path.join(profile, 'extensions', `${UBO}.xpi`);
    expect(existsSync(xpi)).toBe(false);

    await open(page, m, { tool: 'restore', page: 'page-restore' });
    const card = page.getByTestId('restore-backups');
    await expect(card).toContainText('Removed browser add-on: uBlock Origin (Firefox)');
    await expect(card).toContainText('Browser add-on');
    await fits(page);
    await card.locator('[data-testid^="point-restore-"]').click();
    await expect(page.getByTestId('confirm-sheet')).toContainText('browser must be closed');
    await fits(page);
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(page.getByTestId('restore-note')).toContainText('Restored browser add-on uBlock Origin (Firefox).');
    expect(readFileSync(xpi, 'utf8')).toBe('zip-bytes');
    const db = JSON.parse(readFileSync(path.join(profile, 'extensions.json'), 'utf8'));
    expect(db.addons[0].active).toBe(true);
    await fits(page);

    // back on the Browser Plugins page it is listed again
    await open(page, m, { tool: 'plugins', page: 'page-plugins' });
    await expect(page.getByTestId(`plugin-row-${id}`)).toContainText('uBlock Origin');
  });

  test('restoring an add-on while its browser is running is refused and changes nothing', async ({ page, boot }) => {
    let profile = '';
    const m = await boot(
      (mm) => {
        // A profile whose add-on was removed earlier, with the backup that removal left behind.
        profile = firefoxSetup(mm).profile;
        const dbPath = path.join(profile, 'extensions.json');
        const saved = readFileSync(dbPath, 'utf8');
        const backup = path.join(mm.dataDir, 'backups', 'plugins-1700000000');
        write(path.join(backup, 'files', `1-${UBO}.xpi`), 'zip-bytes');
        write(path.join(backup, 'files', '2-extensions.json'), saved);
        write(
          path.join(backup, 'manifest.json'),
          JSON.stringify({
            kind: 'plugins',
            createdAt: '2026-01-01T00:00:00Z',
            description: 'Removed browser add-on: uBlock Origin (Firefox)',
            plugin: {
              action: 'remove',
              browser: 'firefox',
              browserLabel: 'Firefox',
              family: 'firefox',
              profile: 'abcd1234.default-release',
              profileDir: profile,
              extensionId: UBO,
              name: 'uBlock Origin',
            },
            items: [
              { type: 'file', role: 'payload', original: path.join(profile, 'extensions', `${UBO}.xpi`), backup: `files/1-${UBO}.xpi` },
              { type: 'file', role: 'extensions-json', original: dbPath, backup: 'files/2-extensions.json' },
            ],
          }),
        );
        rmSync(path.join(profile, 'extensions', `${UBO}.xpi`));
      },
      { processes: 'firefox|/usr/lib/firefox/firefox|300|1' },
    );
    await open(page, m, { tool: 'restore', page: 'page-restore' });
    await expect(page.getByTestId('restore-backups')).toContainText('Removed browser add-on: uBlock Origin (Firefox)');
    await page.getByTestId('restore-backups').locator('[data-testid^="point-restore-"]').click();
    await page.getByTestId('confirm-sheet-confirm').click();
    await expect(page.getByTestId('error-banner')).toContainText('Close Firefox first');
    await fits(page);
    expect(existsSync(path.join(profile, 'extensions', `${UBO}.xpi`))).toBe(false);
  });
});
