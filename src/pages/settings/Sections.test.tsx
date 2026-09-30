import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RulesListing } from '../../api/cleaner';
import type { Settings } from '../../api/settings';
import type { AgentStatus } from '../../api/smart_cleaning';
import { getLanguage, setLanguage } from '../../i18n';
import { createApiMock } from '../../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock>, tauri: false }));
vi.mock('../../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../../lib/transport')>();
  return {
    ...actual,
    call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o),
    isTauri: () => api.tauri,
  };
});

import SettingsPage from '../SettingsPage';

const defaults: Settings = {
  theme: 'system',
  secureDelete: { enabled: false, passes: 1 },
  closeBrowsers: 'ask',
  tempMinAgeHours: 24,
  include: [],
  exclude: [],
  cookieKeep: [],
  selectedRules: null,
  smart: {
    enabled: false,
    thresholdMb: 500,
    cleanOnBrowserClose: [],
    autoClean: false,
    notify: true,
    checkIntervalMinutes: 60,
    enforceSleepMinutes: 15,
  },
  runAtStartup: false,
  closeToTray: true,
  language: 'en',
};

const listing: RulesListing = {
  categories: [
    {
      category: 'browser',
      label: 'Browsers',
      groups: [
        { group: 'Google Chrome', rules: [{ id: 'chrome.cache', name: 'Cache', description: '', enabled: true, defaultEnabled: true }] },
        { group: 'Mozilla Firefox', rules: [{ id: 'firefox.cache', name: 'Cache', description: '', enabled: true, defaultEnabled: true }] },
      ],
    },
    {
      category: 'system',
      label: 'System',
      groups: [{ group: 'Steam', rules: [{ id: 'steam.x', name: 'x', description: '', enabled: true, defaultEnabled: true }] }],
    },
  ],
};

let server: Settings;
function mergeInto(target: Record<string, unknown>, patch: Record<string, unknown>) {
  for (const [k, v] of Object.entries(patch)) {
    const cur = target[k];
    if (v && typeof v === 'object' && !Array.isArray(v) && cur && typeof cur === 'object' && !Array.isArray(cur)) {
      mergeInto(cur as Record<string, unknown>, v as Record<string, unknown>);
    } else target[k] = v;
  }
}

let agent: AgentStatus;

beforeEach(() => {
  api.tauri = false;
  api.current = createApiMock();
  server = structuredClone(defaults);
  agent = { running: false };
  const h = api.current.handlers;
  h['settings.get'] = () => structuredClone(server);
  h['settings.set'] = (p) => {
    const next = structuredClone(server) as unknown as Record<string, unknown>;
    mergeInto(next, p as Record<string, unknown>);
    server = next as unknown as Settings;
    return structuredClone(server);
  };
  h['cleaner.list_rules'] = () => listing;
  h['smart_cleaning.status'] = () => agent;
  h['system.app_info'] = () => ({ name: 'ClearSweep', version: '9.8.7', license: 'MIT', dataDir: '/home/u/.local/share/clearsweep', os: 'linux' });
  h['system.open_data_dir'] = () => ({ path: '/home/u/.local/share/clearsweep' });
});

afterEach(() => setLanguage('en'));

function renderPage() {
  return render(
    <MemoryRouter>
      <SettingsPage />
    </MemoryRouter>,
  );
}

describe('Smart Cleaning section', () => {
  it('toggles, persists and shows the agent line', async () => {
    renderPage();
    const user = userEvent.setup();
    await waitFor(() =>
      expect(screen.getByTestId('smart-agent-status')).toHaveTextContent(
        'Not running — enable Run at startup or keep ClearSweep open',
      ),
    );
    const enabled = await screen.findByTestId('smart-enabled');
    expect(enabled).toHaveAttribute('aria-checked', 'false');
    await user.click(enabled);
    await waitFor(() => expect(server.smart.enabled).toBe(true));
    await waitFor(() => expect(screen.getByTestId('smart-enabled')).toHaveAttribute('aria-checked', 'true'));
    await user.click(screen.getByTestId('smart-notify'));
    await user.click(screen.getByTestId('smart-auto-clean'));
    await waitFor(() => expect(server.smart).toMatchObject({ notify: false, autoClean: true }));
    expect(api.current.paramsOf('settings.set')).toEqual([
      { smart: { enabled: true } },
      { smart: { notify: false } },
      { smart: { autoClean: true } },
    ]);
  });

  it('says when the background agent is running', async () => {
    agent = { running: true, since: '2024-05-01T10:15:00Z', pid: 4 };
    renderPage();
    await waitFor(() =>
      expect(screen.getByTestId('smart-agent-status')).toHaveTextContent(/^Background agent running since /),
    );
  });

  it('threshold presets and a validated custom value', async () => {
    renderPage();
    const user = userEvent.setup();
    expect(await screen.findByTestId('smart-threshold-500')).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByTestId('smart-threshold-1000')).toHaveTextContent('1 GB');
    await user.click(screen.getByTestId('smart-threshold-2000'));
    await waitFor(() => expect(server.smart.thresholdMb).toBe(2000));
    expect(screen.getByTestId('smart-threshold-custom')).toHaveValue(2000);
    await waitFor(() => expect(screen.getByTestId('smart-threshold-2000')).toHaveAttribute('aria-pressed', 'true'));

    const custom = screen.getByTestId('smart-threshold-custom');
    await user.clear(custom);
    await user.type(custom, '0');
    await user.tab();
    expect(await screen.findByTestId('smart-threshold-error')).toHaveTextContent('whole number of megabytes');
    expect(server.smart.thresholdMb).toBe(2000);
    await user.clear(custom);
    await user.type(custom, '750{enter}');
    await waitFor(() => expect(server.smart.thresholdMb).toBe(750));
    expect(screen.queryByTestId('smart-threshold-error')).toBeNull();
    for (const mb of [100, 500, 1000, 2000]) expect(screen.getByTestId(`smart-threshold-${mb}`)).toHaveAttribute('aria-pressed', 'false');
  });

  it('lists only browsers and saves the ones to clean when they close', async () => {
    renderPage();
    const user = userEvent.setup();
    const chrome = await screen.findByTestId('smart-browser-google-chrome');
    expect(screen.getByTestId('smart-browser-mozilla-firefox')).toBeInTheDocument();
    expect(screen.queryByTestId('smart-browser-steam')).toBeNull();
    await user.click(chrome);
    await user.click(screen.getByTestId('smart-browser-mozilla-firefox'));
    await waitFor(() => expect(server.smart.cleanOnBrowserClose).toEqual(['Google Chrome', 'Mozilla Firefox']));
    await user.click(screen.getByTestId('smart-browser-google-chrome'));
    await waitFor(() => expect(server.smart.cleanOnBrowserClose).toEqual(['Mozilla Firefox']));
  });

  it('check interval, including an unusual saved value', async () => {
    server.smart.checkIntervalMinutes = 45;
    renderPage();
    const user = userEvent.setup();
    const sel = await screen.findByTestId('smart-interval');
    expect(sel).toHaveValue('45');
    await user.selectOptions(sel, '360');
    await waitFor(() => expect(server.smart.checkIntervalMinutes).toBe(360));
  });

  it('still works when the browser list and agent status cannot be loaded', async () => {
    api.current.handlers['cleaner.list_rules'] = () => Promise.reject(new Error('boom'));
    api.current.handlers['smart_cleaning.status'] = () => Promise.reject(new Error('boom'));
    renderPage();
    expect(await screen.findByTestId('smart-enabled')).toBeInTheDocument();
    expect(await screen.findByText('No browsers were found on this computer.')).toBeInTheDocument();
    expect(screen.queryByTestId('error-banner')).toBeNull();
  });
});

describe('Scheduled Cleaning, Startup, Language and About sections', () => {
  it('links to the schedules page', async () => {
    renderPage();
    const link = await screen.findByTestId('settings-schedules-link');
    expect(link).toHaveAttribute('href', '/settings/schedules');
  });

  it('run at startup toggles and persists; close to tray is desktop only', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('setting-run-at-startup'));
    await waitFor(() => expect(server.runAtStartup).toBe(true));
    expect(screen.queryByTestId('setting-close-to-tray')).toBeNull();
  });

  it('shows and changes close to tray in the desktop app', async () => {
    api.tauri = true;
    renderPage();
    const user = userEvent.setup();
    const sw = await screen.findByTestId('setting-close-to-tray');
    expect(sw).toHaveAttribute('aria-checked', 'true');
    await user.click(sw);
    await waitFor(() => expect(server.closeToTray).toBe(false));
  });

  it('a refused run-at-startup change shows the error and leaves the switch off', async () => {
    const { ApiCallError } = await import('../../lib/transport');
    api.current.handlers['settings.set'] = () => Promise.reject(new ApiCallError('Io', 'reg add failed: Access is denied.'));
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('setting-run-at-startup'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('Access is denied');
    expect(screen.getByTestId('setting-run-at-startup')).toHaveAttribute('aria-checked', 'false');
  });

  it('language offers English and persists the choice', async () => {
    renderPage();
    const sel = await screen.findByTestId('setting-language');
    expect(sel).toHaveValue('en');
    expect([...sel.querySelectorAll('option')].map((o) => o.textContent)).toEqual(['English']);
    const user = userEvent.setup();
    await user.selectOptions(sel, 'en');
    expect(getLanguage()).toBe('en');
  });

  it('section titles come from the i18n catalog', async () => {
    renderPage();
    expect(await screen.findByTestId('settings-smart')).toHaveTextContent('Smart Cleaning');
    expect(screen.getByTestId('settings-schedules')).toHaveTextContent('Scheduled Cleaning');
    expect(screen.getByTestId('settings-startup')).toHaveTextContent('Startup & Background');
    expect(screen.getByTestId('settings-language')).toHaveTextContent('Language');
    expect(screen.getByTestId('settings-about')).toHaveTextContent('About');
  });

  it('about shows the name, version, promise, license and opens the data folder', async () => {
    renderPage();
    const user = userEvent.setup();
    await waitFor(() => expect(screen.getByTestId('about-version')).toHaveTextContent('version 9.8.7'));
    expect(screen.getByTestId('about-name')).toHaveTextContent('ClearSweep');
    expect(screen.getByTestId('about-promise')).toHaveTextContent('All features are free. No account, no ads, no telemetry.');
    expect(screen.getByTestId('about-license')).toHaveTextContent('License: MIT');
    expect(screen.getByTestId('about-data-dir')).toHaveTextContent('/home/u/.local/share/clearsweep');
    await user.click(screen.getByTestId('about-open-data'));
    await waitFor(() => expect(api.current.paramsOf('system.open_data_dir')).toHaveLength(1));
    expect(screen.queryByTestId('about-open-error')).toBeNull();
  });

  it('about falls back to the package version and reports a failing folder open', async () => {
    const { ApiCallError } = await import('../../lib/transport');
    api.current.handlers['system.app_info'] = () => Promise.reject(new Error('no'));
    api.current.handlers['system.open_data_dir'] = () =>
      Promise.reject(new ApiCallError('Unsupported', '`xdg-open` was not found'));
    renderPage();
    const user = userEvent.setup();
    expect(await screen.findByTestId('about-version')).toHaveTextContent(/^version \d+\.\d+\.\d+/);
    await user.click(screen.getByTestId('about-open-data'));
    expect(await screen.findByTestId('about-open-error')).toHaveTextContent('xdg-open');
  });
});
