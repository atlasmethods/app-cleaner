import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AnalyzeReport, CleanReport, RulesListing } from '../api/cleaner';
import type { Settings } from '../api/settings';
import { createApiMock, pendingUntilAborted } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import CleanPage from './CleanPage';

const rule = (id: string, name: string, enabled: boolean, warning?: string) => ({
  id,
  name,
  description: `${name} description`,
  enabled,
  defaultEnabled: enabled,
  ...(warning ? { warning } : {}),
});

const listing: RulesListing = {
  categories: [
    {
      category: 'browser',
      label: 'Browsers',
      groups: [
        {
          group: 'Google Chrome',
          rules: [
            rule('chrome.cache', 'Internet Cache', true),
            rule('chrome.history', 'Internet History', true),
            rule('chrome.passwords', 'Saved Passwords', false, 'Deletes every saved password.'),
          ],
        },
        { group: 'Mozilla Firefox', rules: [rule('firefox.cache', 'Internet Cache', false)] },
      ],
    },
    {
      category: 'system',
      label: 'System',
      groups: [{ group: 'System', rules: [rule('linux.temp', 'Temporary Files', true)] }],
    },
  ],
};

const baseSettings: Settings = {
  theme: 'system',
  secureDelete: { enabled: false, passes: 1 },
  closeBrowsers: 'ask',
  tempMinAgeHours: 24,
  include: [],
  exclude: [],
  cookieKeep: [],
  selectedRules: null,
  smart: { enabled: false, thresholdMb: 500, cleanOnBrowserClose: [], autoClean: false, notify: true },
  runAtStartup: false,
  language: 'en',
};

const item = (over: Partial<AnalyzeReport['items'][number]>) => ({
  ruleId: 'x.y',
  name: 'N',
  group: 'G',
  category: 'browser' as const,
  files: 0,
  bytes: 0,
  rows: 0,
  samplePaths: [],
  appRunning: false,
  errors: [],
  actions: [],
  ...over,
});

const analysis: AnalyzeReport = {
  items: [
    item({ ruleId: 'linux.temp', name: 'Temporary Files', group: 'System', category: 'system', files: 2, bytes: 2048, samplePaths: ['/tmp/a', '/tmp/b'] }),
    item({ ruleId: 'chrome.cache', name: 'Internet Cache', group: 'Google Chrome', files: 5, bytes: 104857, samplePaths: ['/home/u/.cache/chrome/x'] }),
    item({ ruleId: 'chrome.history', name: 'Internet History', group: 'Google Chrome', rows: 7, errors: [{ path: '/h/History', message: 'in use' }] }),
    item({ ruleId: 'nothing.here', name: 'Empty', group: 'Nothing' }),
  ],
  totalFiles: 7,
  totalBytes: 106905,
  totalRows: 7,
  durationMs: 5,
};

const cleanOk = (ids: string[]): CleanReport => ({
  results: ids.map((ruleId) => ({
    ruleId,
    removedFiles: 1,
    removedBytes: 100,
    removedRows: 0,
    failed: [],
    actions: [],
    runningApps: [],
    closedApps: [],
  })),
  totalFiles: ids.length,
  totalBytes: 100 * ids.length,
  totalRows: 0,
  durationMs: 3,
  cancelled: false,
  historyId: 'h1',
});

function renderPage() {
  return render(
    <MemoryRouter>
      <CleanPage />
    </MemoryRouter>,
  );
}

async function expandChrome(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByTestId('group-expand-google-chrome'));
}

beforeEach(() => {
  api.current = createApiMock();
  api.current.handlers['cleaner.list_rules'] = () => listing;
  api.current.handlers['settings.get'] = () => baseSettings;
  api.current.handlers['settings.set'] = (p) => ({ ...baseSettings, ...(p as object) });
});

describe('CleanPage selection', () => {
  it('groups rules by category and group with tri-state group checkboxes', async () => {
    renderPage();
    expect(await screen.findByTestId('category-browser')).toBeInTheDocument();
    expect(screen.getByTestId('category-system')).toBeInTheDocument();
    const chrome = screen.getByTestId('group-check-google-chrome') as HTMLInputElement;
    expect(chrome.indeterminate).toBe(true);
    expect((screen.getByTestId('group-check-mozilla-firefox') as HTMLInputElement).checked).toBe(false);
    expect((screen.getByTestId('group-check-system') as HTMLInputElement).checked).toBe(true);
    // rules are hidden until the group is expanded
    expect(screen.queryByTestId('rule-chrome.cache')).toBeNull();
    const user = userEvent.setup();
    await expandChrome(user);
    expect((screen.getByTestId('rule-chrome.cache') as HTMLInputElement).checked).toBe(true);
    expect((screen.getByTestId('rule-chrome.passwords') as HTMLInputElement).checked).toBe(false);
    expect(screen.getByTestId('group-expand-google-chrome')).toHaveTextContent('2/3');
  });

  it('persists a single rule toggle as the full ordered selection', async () => {
    renderPage();
    const user = userEvent.setup();
    await expandChrome(user);
    await user.click(screen.getByTestId('rule-chrome.history'));
    await waitFor(() => expect(api.current.paramsOf('settings.set')).toHaveLength(1));
    expect(api.current.paramsOf('settings.set')[0]).toEqual({ selectedRules: ['chrome.cache', 'linux.temp'] });
    expect((screen.getByTestId('rule-chrome.history') as HTMLInputElement).checked).toBe(false);
    expect((screen.getByTestId('group-check-google-chrome') as HTMLInputElement).indeterminate).toBe(true);
  });

  it('turning on a group with a warned rule asks first, and cancel changes nothing', async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('group-check-google-chrome');
    await user.click(screen.getByTestId('group-check-google-chrome'));
    const sheet = await screen.findByTestId('confirm-sheet');
    expect(sheet).toHaveTextContent('Deletes every saved password.');
    await user.click(within(sheet).getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByTestId('confirm-sheet')).toBeNull();
    expect(api.current.paramsOf('settings.set')).toHaveLength(0);
    expect((screen.getByTestId('group-check-google-chrome') as HTMLInputElement).indeterminate).toBe(true);

    await user.click(screen.getByTestId('group-check-google-chrome'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('settings.set')).toHaveLength(1));
    expect(api.current.paramsOf('settings.set')[0]).toEqual({
      selectedRules: ['chrome.cache', 'chrome.history', 'chrome.passwords', 'linux.temp'],
    });
    const chrome = screen.getByTestId('group-check-google-chrome') as HTMLInputElement;
    expect(chrome.checked).toBe(true);
    expect(chrome.indeterminate).toBe(false);
  });

  it('enabling a single warned rule shows its warning; disabling never asks', async () => {
    renderPage();
    const user = userEvent.setup();
    await expandChrome(user);
    await user.click(screen.getByTestId('rule-chrome.passwords'));
    expect(await screen.findByTestId('confirm-sheet')).toHaveTextContent('Saved Passwords');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('rule-chrome.passwords')).toBeChecked());
    await user.click(screen.getByTestId('rule-chrome.passwords'));
    expect(screen.queryByTestId('confirm-sheet')).toBeNull();
    await waitFor(() => expect(screen.getByTestId('rule-chrome.passwords')).not.toBeChecked());
  });

  it('a fully selected group switches everything off', async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('group-check-system');
    await user.click(screen.getByTestId('group-check-system'));
    await waitFor(() => expect(api.current.paramsOf('settings.set')).toHaveLength(1));
    expect(api.current.paramsOf('settings.set')[0]).toEqual({ selectedRules: ['chrome.cache', 'chrome.history'] });
    expect(screen.getByTestId('btn-analyze')).toBeEnabled();
  });

  it('disables Analyze when nothing is selected', async () => {
    api.current.handlers['cleaner.list_rules'] = () => ({
      categories: [{ category: 'system', label: 'System', groups: [{ group: 'System', rules: [rule('a.b', 'A', false)] }] }],
    });
    renderPage();
    await screen.findByTestId('btn-analyze');
    expect(screen.getByTestId('btn-analyze')).toBeDisabled();
    expect(screen.getByTestId('btn-clean')).toBeDisabled();
  });

  it('shows an error banner when the rules cannot be loaded', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['cleaner.list_rules'] = () => Promise.reject(new ApiCallError('Io', 'disk on fire'));
    renderPage();
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('disk on fire');
  });
});

describe('CleanPage analyze and clean', () => {
  it('analyzes the selection and lists results biggest first with expandable details', async () => {
    api.current.handlers['cleaner.analyze'] = (_p, o) => {
      (o as { onProgress?: (e: object) => void }).onProgress?.({ stage: 'analyze', fraction: 0.5, message: 'Halfway' });
      return analysis;
    };
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('btn-analyze');
    expect(screen.getByTestId('btn-clean')).toBeDisabled();
    await user.click(screen.getByTestId('btn-analyze'));
    await screen.findByTestId('results');
    expect(api.current.paramsOf('cleaner.analyze')[0]).toEqual({
      ruleIds: ['chrome.cache', 'chrome.history', 'linux.temp'],
    });
    const rows = screen.getAllByTestId(/^result-[a-z.]+$/);
    expect(rows.map((r) => r.getAttribute('data-testid'))).toEqual([
      'result-chrome.cache',
      'result-linux.temp',
      'result-chrome.history',
    ]);
    expect(screen.queryByTestId('result-nothing.here')).toBeNull();
    expect(screen.getByTestId('result-chrome.cache-size')).toHaveTextContent('102 KB');
    expect(screen.getByTestId('result-chrome.cache')).toHaveTextContent('5 files');
    expect(screen.getByTestId('result-chrome.history')).toHaveTextContent('7 entries');
    expect(screen.getByTestId('results-total')).toHaveTextContent('104 KB');

    expect(screen.queryByTestId('result-chrome.cache-details')).toBeNull();
    await user.click(screen.getByTestId('result-chrome.cache-toggle'));
    expect(screen.getByTestId('result-chrome.cache-details')).toHaveTextContent('/home/u/.cache/chrome/x');
    await user.click(screen.getByTestId('result-chrome.history-toggle'));
    expect(screen.getByTestId('result-chrome.history-errors')).toHaveTextContent('/h/History: in use');
    expect(screen.getByTestId('btn-clean')).toBeEnabled();
  });

  it('shows progress with a cancel button that aborts the analysis', async () => {
    api.current.handlers['cleaner.analyze'] = (_p, o) => {
      (o as { onProgress?: (e: object) => void }).onProgress?.({ stage: 'analyze', fraction: 0.25, message: 'Google Chrome - Cache' });
      return pendingUntilAborted(o as { signal?: AbortSignal });
    };
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('btn-analyze');
    await user.click(screen.getByTestId('btn-analyze'));
    const bar = await screen.findByRole('progressbar', { name: 'Analysis progress' });
    expect(bar).toHaveAttribute('aria-valuenow', '25');
    expect(screen.getByTestId('clean-progress-message')).toHaveTextContent('Google Chrome - Cache');
    expect(screen.getByTestId('btn-analyze')).toBeDisabled();
    await user.click(screen.getByTestId('btn-cancel'));
    await waitFor(() => expect(screen.queryByTestId('clean-progress')).toBeNull());
    expect(screen.queryByTestId('error-banner')).toBeNull();
    expect(screen.getByTestId('btn-analyze')).toBeEnabled();
  });

  it('marks results stale when the selection changes and blocks cleaning', async () => {
    api.current.handlers['cleaner.analyze'] = () => analysis;
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('btn-analyze');
    await user.click(screen.getByTestId('btn-analyze'));
    await screen.findByTestId('results');
    await user.click(screen.getByTestId('group-check-mozilla-firefox'));
    expect(await screen.findByTestId('results-stale')).toBeInTheDocument();
    expect(screen.getByTestId('btn-clean')).toBeDisabled();
    await user.click(screen.getByTestId('btn-analyze'));
    await waitFor(() => expect(screen.queryByTestId('results-stale')).toBeNull());
    expect(screen.getByTestId('btn-clean')).toBeEnabled();
  });

  it('confirms with the totals, cleans only rules that have content, and shows a summary', async () => {
    api.current.handlers['cleaner.analyze'] = () => analysis;
    api.current.handlers['cleaner.clean'] = () => cleanOk(['chrome.cache', 'linux.temp', 'chrome.history']);
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('btn-analyze');
    await user.click(screen.getByTestId('btn-analyze'));
    await screen.findByTestId('results');
    await user.click(screen.getByTestId('btn-clean'));
    const sheet = await screen.findByTestId('confirm-sheet');
    expect(sheet).toHaveTextContent('This will permanently delete 7 files (104 KB) and 7 browser database entries.');
    expect(api.current.paramsOf('cleaner.clean')).toHaveLength(0);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('clean-summary')).toBeInTheDocument();
    expect(api.current.paramsOf('cleaner.clean')[0]).toEqual({
      ruleIds: ['linux.temp', 'chrome.cache', 'chrome.history'],
    });
    expect(screen.getByTestId('clean-summary-total')).toHaveTextContent('Removed 300 B');
    expect(screen.getByTestId('summary-chrome.cache')).toHaveTextContent('Google Chrome - Internet Cache');
    expect(screen.getByTestId('history-link')).toHaveAttribute('href', '/clean/history');
    // results of the old analysis are gone: it is stale after a clean
    expect(screen.queryByTestId('results')).toBeNull();
    expect(screen.getByTestId('btn-clean')).toBeDisabled();
  });

  it('cancelling the confirmation deletes nothing', async () => {
    api.current.handlers['cleaner.analyze'] = () => analysis;
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('btn-analyze');
    await user.click(screen.getByTestId('btn-analyze'));
    await screen.findByTestId('results');
    await user.click(screen.getByTestId('btn-clean'));
    await user.click(within(await screen.findByTestId('confirm-sheet')).getByRole('button', { name: 'Cancel' }));
    expect(api.current.paramsOf('cleaner.clean')).toHaveLength(0);
    expect(screen.queryByTestId('clean-summary')).toBeNull();
  });

  it('nothing to clean: shows the empty state and keeps Clean disabled', async () => {
    api.current.handlers['cleaner.analyze'] = () => ({ ...analysis, items: [item({})], totalFiles: 0, totalBytes: 0, totalRows: 0 });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('btn-analyze');
    await user.click(screen.getByTestId('btn-analyze'));
    expect(await screen.findByTestId('results-empty')).toBeInTheDocument();
    expect(screen.getByTestId('btn-clean')).toBeDisabled();
  });

  it('lists running apps, retries just those rules with closeApps=always and merges the summary', async () => {
    api.current.handlers['cleaner.analyze'] = () => analysis;
    const first: CleanReport = {
      ...cleanOk(['linux.temp']),
      results: [
        ...cleanOk(['linux.temp']).results,
        {
          ruleId: 'chrome.cache',
          removedFiles: 0,
          removedBytes: 0,
          removedRows: 0,
          failed: [],
          skipped: 'app_running',
          actions: [],
          runningApps: ['Google Chrome'],
          closedApps: [],
        },
        {
          ruleId: 'chrome.history',
          removedFiles: 0,
          removedBytes: 0,
          removedRows: 0,
          failed: [],
          skipped: 'app_running',
          actions: [],
          runningApps: ['Google Chrome'],
          closedApps: [],
        },
      ],
    };
    const retry: CleanReport = {
      ...cleanOk(['chrome.cache', 'chrome.history']),
      results: cleanOk(['chrome.cache', 'chrome.history']).results.map((r) => ({ ...r, closedApps: ['Google Chrome'] })),
    };
    let n = 0;
    api.current.handlers['cleaner.clean'] = () => (n++ === 0 ? first : retry);
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('btn-analyze');
    await user.click(screen.getByTestId('btn-analyze'));
    await screen.findByTestId('results');
    await user.click(screen.getByTestId('btn-clean'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    const sheet = await screen.findByText('Close running apps?');
    expect(sheet).toBeInTheDocument();
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Google Chrome is running');
    expect(screen.getByTestId('clean-summary-skipped')).toHaveTextContent('Google Chrome is running');
    await user.click(screen.getByRole('button', { name: 'Close apps & clean' }));
    await waitFor(() => expect(api.current.paramsOf('cleaner.clean')).toHaveLength(2));
    expect(api.current.paramsOf('cleaner.clean')[1]).toEqual({
      ruleIds: ['chrome.cache', 'chrome.history'],
      closeApps: 'always',
    });
    await waitFor(() => expect(screen.queryByTestId('clean-summary-skipped')).toBeNull());
    expect(screen.getByTestId('summary-chrome.cache')).toHaveTextContent('closed Google Chrome');
    expect(screen.getByTestId('clean-summary-total')).toHaveTextContent('Removed 300 B');
  });

  it('Skip keeps the apps running and leaves the skipped rules in the summary', async () => {
    api.current.handlers['cleaner.analyze'] = () => analysis;
    api.current.handlers['cleaner.clean'] = () => ({
      ...cleanOk([]),
      results: [
        {
          ruleId: 'chrome.cache',
          removedFiles: 0,
          removedBytes: 0,
          removedRows: 0,
          failed: [],
          skipped: 'app_running',
          actions: [],
          runningApps: ['Google Chrome'],
          closedApps: [],
        },
      ],
    });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('btn-analyze');
    await user.click(screen.getByTestId('btn-analyze'));
    await screen.findByTestId('results');
    await user.click(screen.getByTestId('btn-clean'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await screen.findByText('Close running apps?');
    await user.click(screen.getByRole('button', { name: 'Skip' }));
    expect(screen.queryByTestId('confirm-sheet')).toBeNull();
    expect(api.current.paramsOf('cleaner.clean')).toHaveLength(1);
    expect(screen.getByTestId('clean-summary-skipped')).toHaveTextContent(
      'Google Chrome - Internet Cache: skipped - Google Chrome is running',
    );
  });

  it('does not ask about apps when the policy is not "ask"', async () => {
    api.current.handlers['settings.get'] = () => ({ ...baseSettings, closeBrowsers: 'skip' });
    api.current.handlers['cleaner.analyze'] = () => analysis;
    api.current.handlers['cleaner.clean'] = () => ({
      ...cleanOk([]),
      results: [
        {
          ruleId: 'chrome.cache',
          removedFiles: 0,
          removedBytes: 0,
          removedRows: 0,
          failed: [],
          skipped: 'app_running',
          actions: [],
          runningApps: ['Google Chrome'],
          closedApps: [],
        },
      ],
    });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('btn-analyze');
    await user.click(screen.getByTestId('btn-analyze'));
    await screen.findByTestId('results');
    await user.click(screen.getByTestId('btn-clean'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await screen.findByTestId('clean-summary');
    expect(screen.queryByTestId('confirm-sheet')).toBeNull();
  });
});
