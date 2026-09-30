import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Settings } from '../api/settings';
import { createApiMock } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import SettingsPage from './SettingsPage';

const defaults: Settings = {
  theme: 'system',
  secureDelete: { enabled: false, passes: 1 },
  closeBrowsers: 'ask',
  tempMinAgeHours: 24,
  include: [],
  exclude: [],
  cookieKeep: [],
  selectedRules: null,
  smart: { enabled: false, thresholdMb: 500, cleanOnBrowserClose: [], autoClean: false, notify: true, checkIntervalMinutes: 60, enforceSleepMinutes: 15 },
  runAtStartup: false,
  language: 'en',
};

/** A tiny in-memory settings server implementing merge-patch for the fields the UI touches. */
let server: Settings;
function mergeInto(target: Record<string, unknown>, patch: Record<string, unknown>) {
  for (const [k, v] of Object.entries(patch)) {
    const cur = target[k];
    if (v && typeof v === 'object' && !Array.isArray(v) && cur && typeof cur === 'object' && !Array.isArray(cur)) {
      mergeInto(cur as Record<string, unknown>, v as Record<string, unknown>);
    } else target[k] = v;
  }
}

function renderPage() {
  return render(
    <MemoryRouter>
      <SettingsPage />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  api.current = createApiMock();
  server = structuredClone(defaults);
  api.current.handlers['settings.get'] = () => structuredClone(server);
  api.current.handlers['settings.set'] = (p) => {
    const next = structuredClone(server) as unknown as Record<string, unknown>;
    mergeInto(next, p as Record<string, unknown>);
    server = next as unknown as Settings;
    return structuredClone(server);
  };
});

afterEach(() => {
  document.documentElement.removeAttribute('data-theme');
  window.localStorage.clear();
});

describe('SettingsPage', () => {
  it('renders every section', async () => {
    renderPage();
    for (const id of ['appearance', 'cleaning', 'include', 'exclude', 'smart', 'schedules', 'startup', 'language', 'about']) {
      expect(await screen.findByTestId(`settings-${id}`)).toBeInTheDocument();
    }
  });

  it('applies the theme immediately and persists it', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('theme-dark'));
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
    await waitFor(() => expect(screen.getByTestId('theme-dark')).toHaveAttribute('aria-checked', 'true'));
    expect(api.current.paramsOf('settings.set')[0]).toEqual({ theme: 'dark' });
    await user.click(screen.getByTestId('theme-system'));
    expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
    await user.click(screen.getByTestId('theme-light'));
    expect(document.documentElement.getAttribute('data-theme')).toBe('light');
    expect(server.theme).toBe('light');
  });

  it('compact mode applies at once, is remembered locally and saved', async () => {
    const { getCompactMode, _resetLayoutForTests } = await import('../lib/layout');
    renderPage();
    const user = userEvent.setup();
    const sw = await screen.findByTestId('setting-compact-mode');
    expect(sw).toHaveAttribute('aria-checked', 'false');
    await user.click(sw);
    expect(getCompactMode()).toBe(true);
    expect(window.localStorage.getItem('clearsweep.compact')).toBe('1');
    await waitFor(() => expect(server.compactMode).toBe(true));
    expect(api.current.paramsOf('settings.set').at(-1)).toEqual({ compactMode: true });
    await waitFor(() => expect(screen.getByTestId('setting-compact-mode')).toHaveAttribute('aria-checked', 'true'));
    await user.click(screen.getByTestId('setting-compact-mode'));
    expect(getCompactMode()).toBe(false);
    _resetLayoutForTests();
  });

  it('secure deletion toggle and passes', async () => {
    renderPage();
    const user = userEvent.setup();
    const passes = await screen.findByTestId('setting-secure-passes');
    expect(passes).toBeDisabled();
    await user.click(screen.getByTestId('setting-secure-enabled'));
    await waitFor(() => expect(passes).toBeEnabled());
    await user.selectOptions(passes, '7');
    await waitFor(() => expect(server.secureDelete).toEqual({ enabled: true, passes: 7 }));
    expect(api.current.paramsOf('settings.set')).toEqual([
      { secureDelete: { enabled: true } },
      { secureDelete: { passes: 7 } },
    ]);
  });

  it('close-browsers policy and temp age, with validation', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.selectOptions(await screen.findByTestId('setting-close-browsers'), 'always');
    await waitFor(() => expect(server.closeBrowsers).toBe('always'));
    const age = screen.getByTestId('setting-temp-age');
    await user.clear(age);
    await user.type(age, '-3');
    await user.tab();
    expect(await screen.findByRole('alert')).toHaveTextContent('whole number of hours');
    expect(server.tempMinAgeHours).toBe(24);
    await user.clear(age);
    await user.type(age, '48');
    await user.tab();
    await waitFor(() => expect(server.tempMinAgeHours).toBe(48));
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('adds and removes include entries', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.type(await screen.findByTestId('include-path'), '~/scratch');
    await user.clear(screen.getByTestId('include-mask'));
    await user.type(screen.getByTestId('include-mask'), '*.tmp');
    await user.click(screen.getByTestId('include-empty-dirs'));
    await user.click(screen.getByTestId('include-add'));
    await waitFor(() => expect(server.include).toHaveLength(1));
    const e = server.include[0]!;
    expect(e).toMatchObject({ path: '~/scratch', mask: '*.tmp', recursive: true, removeEmptyDirs: true });
    expect(e.id.length).toBeGreaterThan(8);
    expect(screen.getByTestId(`include-${e.id}`)).toHaveTextContent('~/scratch');
    expect(screen.getByTestId('include-path')).toHaveValue('');
    await user.click(screen.getByTestId(`include-remove-${e.id}`));
    await waitFor(() => expect(server.include).toHaveLength(0));
    expect(screen.queryByTestId(`include-${e.id}`)).toBeNull();
  });

  it('shows the server error for a rejected include and keeps the form', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['settings.set'] = () => Promise.reject(new ApiCallError('InvalidParams', '`~` is a protected folder'));
    renderPage();
    const user = userEvent.setup();
    await user.type(await screen.findByTestId('include-path'), '~');
    await user.click(screen.getByTestId('include-add'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('protected folder');
    expect(screen.getByTestId('include-path')).toHaveValue('~');
  });

  it('adds and removes exclusions', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.type(await screen.findByTestId('exclude-pattern'), '~/Downloads/keep*{enter}');
    await waitFor(() => expect(server.exclude).toHaveLength(1));
    expect(server.exclude[0]!.pattern).toBe('~/Downloads/keep*');
    const id = server.exclude[0]!.id;
    expect(await screen.findByTestId(`exclude-${id}`)).toHaveTextContent('~/Downloads/keep*');
    await user.click(screen.getByTestId(`exclude-remove-${id}`));
    await waitFor(() => expect(server.exclude).toHaveLength(0));
  });

  it('shows an error banner when settings cannot be loaded', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['settings.get'] = () => Promise.reject(new ApiCallError('Io', 'cannot read'));
    renderPage();
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('cannot read');
  });
});
