import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { FixReport, HealthReport } from '../api/health';
import { baseSettings, fixReport, report100, report72, securityCat } from '../test/healthFixtures';
import { createApiMock, pendingUntilAborted } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import HomePage from './HomePage';

type Opts = { signal?: AbortSignal; onProgress?: (e: object) => void };

function renderPage() {
  return render(
    <MemoryRouter>
      <HomePage />
    </MemoryRouter>,
  );
}

function setup(over: { last?: HealthReport | null; settings?: object } = {}) {
  api.current.handlers['health.last'] = () => (over.last === undefined ? null : over.last);
  api.current.handlers['settings.get'] = () => ({ ...baseSettings, ...(over.settings ?? {}) });
}

async function scanned(user: ReturnType<typeof userEvent.setup>, report: HealthReport = report72) {
  api.current.handlers['health.analyze'] = () => report;
  await screen.findByTestId('btn-scan');
  await user.click(screen.getByTestId('btn-scan'));
  await screen.findByTestId('cat-privacy');
  await waitFor(() => expect(screen.getByTestId('health-status')).not.toHaveTextContent('Not scanned'));
}

beforeEach(() => {
  api.current = createApiMock();
});

describe('Home: never scanned', () => {
  it('shows an empty ring, an invitation and only the Scan button', async () => {
    setup();
    renderPage();
    expect(await screen.findByTestId('page-home')).toBeInTheDocument();
    expect(screen.getByRole('img', { name: 'Health score not measured yet' })).toBeInTheDocument();
    expect(screen.getByTestId('health-status')).toHaveTextContent('Not scanned yet');
    expect(screen.getByTestId('btn-scan')).toHaveTextContent('Scan');
    expect(screen.getByTestId('btn-scan')).not.toHaveTextContent('Rescan');
    expect(screen.queryByTestId('health-categories')).toBeNull();
    expect(screen.queryByTestId('btn-fix-all')).toBeNull();
    // asking for the last result must never start a scan
    expect(api.current.paramsOf('health.analyze')).toHaveLength(0);
  });

  it('has quick links to Clean, Performance and the Software Updater', async () => {
    setup();
    renderPage();
    await screen.findByTestId('quick-links');
    expect(screen.getByTestId('link-clean')).toHaveAttribute('href', '/clean');
    expect(screen.getByTestId('link-performance')).toHaveAttribute('href', '/performance');
    expect(screen.getByTestId('link-updater')).toHaveAttribute('href', '/tools/updater');
  });
});

describe('Home: the previous scan', () => {
  it('shows the last score at once, dimmed, with a Rescan button and nothing fixable', async () => {
    setup({ last: report72 });
    renderPage();
    expect(await screen.findByRole('img', { name: 'Health score 72 of 100' })).toBeInTheDocument();
    expect(screen.getByTestId('health-scanned')).toHaveTextContent('Last scan');
    expect(screen.getByTestId('health-stale')).toBeInTheDocument();
    expect(screen.getByTestId('btn-scan')).toHaveTextContent('Rescan');
    expect(screen.getByTestId('cat-space-summary')).toHaveTextContent('1.2 GB of junk in 214 files.');
    // a stale report cannot be fixed, and its sheets are read-only
    expect(screen.queryByTestId('btn-fix-all')).toBeNull();
    const user = userEvent.setup();
    await user.click(screen.getByTestId('cat-security'));
    expect(await screen.findByTestId('sheet-stale')).toBeInTheDocument();
    expect(screen.getByTestId('sel-update-apt:firefox')).toBeDisabled();
    expect(api.current.paramsOf('health.analyze')).toHaveLength(0);
  });
});

describe('Home: scanning', () => {
  it('shows progress with a cancel button and keeps what was shown before', async () => {
    setup({ last: report72 });
    api.current.handlers['health.analyze'] = (_p, o) => {
      (o as Opts).onProgress?.({ stage: 'health', fraction: 0.5, message: 'Checking space' });
      return pendingUntilAborted(o as Opts);
    };
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('btn-scan');
    await user.click(screen.getByTestId('btn-scan'));
    const bar = await screen.findByRole('progressbar', { name: 'Scanning progress' });
    expect(bar).toHaveAttribute('aria-valuenow', '50');
    expect(screen.getByTestId('health-progress-message')).toHaveTextContent('Checking space');
    expect(screen.getByTestId('btn-scan')).toBeDisabled();
    await user.click(screen.getByTestId('btn-cancel'));
    await waitFor(() => expect(screen.queryByTestId('health-progress')).toBeNull());
    expect(screen.queryByTestId('error-banner')).toBeNull();
    expect(screen.getByRole('img', { name: 'Health score 72 of 100' })).toBeInTheDocument();
    expect(screen.getByTestId('btn-scan')).toBeEnabled();
  });

  it('reports a failed scan', async () => {
    setup();
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['health.analyze'] = () => Promise.reject(new ApiCallError('Io', 'disk on fire'));
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('btn-scan');
    await user.click(screen.getByTestId('btn-scan'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('disk on fire');
  });
});

describe('Home: results', () => {
  it('shows the score, the status line and the four categories', async () => {
    setup();
    renderPage();
    const user = userEvent.setup();
    await scanned(user);
    expect(screen.getByRole('img', { name: 'Health score 72 of 100' })).toBeInTheDocument();
    expect(screen.getByTestId('health-status')).toHaveTextContent('Fair - 4 areas need attention');
    expect(screen.getByTestId('btn-scan')).toHaveTextContent('Rescan');
    const cards = screen.getAllByTestId(/^cat-[a-z]+$/);
    expect(cards.map((c) => c.getAttribute('data-testid'))).toEqual([
      'cat-privacy',
      'cat-space',
      'cat-speed',
      'cat-security',
    ]);
    expect(screen.getByTestId('cat-privacy-status')).toHaveTextContent('Warning');
    expect(screen.getByTestId('cat-space-status')).toHaveTextContent('Problem');
    expect(screen.getByTestId('cat-security-summary')).toHaveTextContent('3 updates available, 1 security.');
    expect(screen.queryByTestId('health-stale')).toBeNull();
    expect(screen.getByTestId('btn-fix-all')).toBeEnabled();
  });

  it('shows an unavailable category with its reason, and offers no fix for it', async () => {
    setup();
    renderPage();
    const user = userEvent.setup();
    await scanned(user, {
      ...report72,
      categories: [
        report72.categories[0]!,
        report72.categories[1]!,
        report72.categories[2]!,
        { ...securityCat, status: 'unavailable', summary: 'No supported package manager was found', fixable: false, findings: [] },
      ],
    });
    expect(screen.getByTestId('cat-security-status')).toHaveTextContent('Unavailable');
    await user.click(screen.getByTestId('cat-security'));
    expect(await screen.findByTestId('sheet-unavailable')).toHaveTextContent('No supported package manager was found');
    await user.click(screen.getByTestId('health-sheet-done'));
    await user.click(screen.getByTestId('btn-fix-all'));
    expect(screen.getByTestId('confirm-sheet')).not.toHaveTextContent('update');
  });

  it('is all clear when nothing needs fixing', async () => {
    setup();
    renderPage();
    const user = userEvent.setup();
    await scanned(user, report100);
    expect(screen.getByRole('img', { name: 'Health score 100 of 100' })).toBeInTheDocument();
    expect(screen.getByTestId('health-status')).toHaveTextContent('Excellent - nothing needs attention');
    // nothing to fix: no Fix all button at all
    expect(screen.queryByTestId('btn-fix-all')).toBeNull();
  });
});

describe('Home: selection', () => {
  it('ticks every fixable finding except non-security updates', async () => {
    setup();
    renderPage();
    const user = userEvent.setup();
    await scanned(user);

    await user.click(screen.getByTestId('cat-speed'));
    const sheet = await screen.findByTestId('health-sheet');
    for (const id of ['slack', 'spotify', 'dropbox', 'steam']) {
      expect(within(sheet).getByTestId(`sel-startup-xdg:user:${id}.desktop`)).toBeChecked();
    }
    expect(within(sheet).getByTestId('sel-app-slack')).toBeChecked();
    expect(within(sheet).getByTestId('sel-app-spotify')).toBeChecked();
    await user.click(screen.getByTestId('health-sheet-done'));

    await user.click(screen.getByTestId('cat-security'));
    expect(await screen.findByTestId('sel-update-apt:firefox')).toBeChecked();
    expect(screen.getByTestId('sel-update-apt:vim')).not.toBeChecked();
    expect(screen.getByTestId('sel-update-apt:htop')).not.toBeChecked();
    expect(screen.getByTestId('health-sheet')).toHaveTextContent('Security');
    await user.click(screen.getByTestId('health-sheet-done'));

    await user.click(screen.getByTestId('cat-privacy'));
    expect(await screen.findByTestId('sel-privacy')).toBeChecked();
    await user.click(screen.getByTestId('health-sheet-done'));
    await user.click(screen.getByTestId('cat-space'));
    expect(await screen.findByTestId('sel-space')).toBeChecked();
    expect(screen.getByTestId('sheet-junk')).toHaveTextContent('Google Chrome');
  });

  it('changes what Fix all will do, and disables it when nothing is left', async () => {
    setup();
    renderPage();
    const user = userEvent.setup();
    await scanned(user);
    await user.click(screen.getByTestId('cat-security'));
    await user.click(await screen.findByTestId('sel-update-apt:vim'));
    expect(screen.getByTestId('sel-update-apt:vim')).toBeChecked();
    await user.click(screen.getByTestId('updates-none'));
    expect(screen.getByTestId('sel-update-apt:firefox')).not.toBeChecked();
    await user.click(screen.getByTestId('updates-all'));
    expect(screen.getByTestId('sel-update-apt:htop')).toBeChecked();
    await user.click(screen.getByTestId('updates-security-only'));
    expect(screen.getByTestId('sel-update-apt:htop')).not.toBeChecked();
    expect(screen.getByTestId('sel-update-apt:firefox')).toBeChecked();
    await user.click(screen.getByTestId('health-sheet-done'));

    // untick everything
    await user.click(screen.getByTestId('cat-security'));
    await user.click(await screen.findByTestId('updates-none'));
    await user.click(screen.getByTestId('health-sheet-done'));
    await user.click(screen.getByTestId('cat-speed'));
    for (const id of ['slack', 'spotify', 'dropbox', 'steam']) {
      await user.click(await screen.findByTestId(`sel-startup-xdg:user:${id}.desktop`));
    }
    await user.click(screen.getByTestId('sel-app-slack'));
    await user.click(screen.getByTestId('sel-app-spotify'));
    await user.click(screen.getByTestId('health-sheet-done'));
    await user.click(screen.getByTestId('cat-privacy'));
    await user.click(await screen.findByTestId('sel-privacy'));
    await user.click(screen.getByTestId('health-sheet-done'));
    await user.click(screen.getByTestId('cat-space'));
    await user.click(await screen.findByTestId('sel-space'));
    await user.click(screen.getByTestId('health-sheet-done'));
    expect(screen.getByTestId('btn-fix-all')).toBeDisabled();
  });
});

describe('Home: confirm and fix', () => {
  it('summarises exactly what will happen before doing anything', async () => {
    setup();
    api.current.handlers['health.fix'] = () => fixReport();
    renderPage();
    const user = userEvent.setup();
    await scanned(user);
    await user.click(screen.getByTestId('btn-fix-all'));
    const sheet = await screen.findByTestId('confirm-sheet');
    expect(sheet).toHaveTextContent(
      'Delete 1.2 GB of junk, remove tracking cookies from 3 browsers, disable 4 startup items, put 2 apps to sleep, install 1 update.',
    );
    expect(sheet).toHaveTextContent('Deleted files cannot be restored.');
    expect(sheet).toHaveTextContent('save your work first');
    expect(api.current.paramsOf('health.fix')).toHaveLength(0);
    await user.click(within(sheet).getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByTestId('confirm-sheet')).toBeNull();
    expect(api.current.paramsOf('health.fix')).toHaveLength(0);
  });

  it('sends exactly the reviewed selection, then shows the new score and what was done', async () => {
    setup();
    api.current.handlers['health.fix'] = () => fixReport();
    renderPage();
    const user = userEvent.setup();
    await scanned(user);
    // the user also wants the vim update and does not want Steam disabled
    await user.click(screen.getByTestId('cat-security'));
    await user.click(await screen.findByTestId('sel-update-apt:vim'));
    await user.click(screen.getByTestId('health-sheet-done'));
    await user.click(screen.getByTestId('cat-speed'));
    await user.click(await screen.findByTestId('sel-startup-xdg:user:steam.desktop'));
    await user.click(screen.getByTestId('health-sheet-done'));

    await user.click(screen.getByTestId('btn-fix-all'));
    expect(await screen.findByTestId('confirm-sheet')).toHaveTextContent(
      'Delete 1.2 GB of junk, remove tracking cookies from 3 browsers, disable 3 startup items, put 2 apps to sleep, install 2 updates.',
    );
    await user.click(screen.getByTestId('confirm-sheet-confirm'));

    expect(await screen.findByTestId('health-result')).toBeInTheDocument();
    expect(api.current.paramsOf('health.fix')).toEqual([
      {
        privacy: true,
        space: true,
        startupIds: ['xdg:user:slack.desktop', 'xdg:user:spotify.desktop', 'xdg:user:dropbox.desktop'],
        sleepAppIds: ['slack', 'spotify'],
        updateIds: ['apt:firefox', 'apt:vim'],
        closeApps: 'skip',
      },
    ]);
    expect(screen.getByTestId('health-result-title')).toHaveTextContent('All done');
    expect(screen.getByTestId('health-result-score')).toHaveTextContent('Score 72 → 100');
    expect(screen.getByTestId('done-privacy')).toHaveTextContent('Removed 12 browser entries');
    expect(screen.getByTestId('done-space')).toHaveTextContent('Deleted 1.2 GB (214 files)');
    expect(screen.getByRole('img', { name: 'Health score 100 of 100' })).toBeInTheDocument();
    // the categories now show the fresh state; nothing left to fix
    expect(screen.getByTestId('cat-space-status')).toHaveTextContent('Good');
    expect(screen.queryByTestId('btn-fix-all')).toBeNull();
    expect(screen.queryByTestId('health-blocked')).toBeNull();
  });

  it('passes closeApps "always" when the Clean settings say browsers may be closed', async () => {
    setup({ settings: { closeBrowsers: 'always' } });
    api.current.handlers['health.fix'] = () => fixReport();
    renderPage();
    const user = userEvent.setup();
    await scanned(user);
    await waitFor(() => expect(api.current.paramsOf('settings.get').length).toBeGreaterThan(0));
    await user.click(screen.getByTestId('btn-fix-all'));
    await user.click(await screen.findByTestId('confirm-sheet-confirm'));
    await screen.findByTestId('health-result');
    expect((api.current.paramsOf('health.fix')[0] as { closeApps: string }).closeApps).toBe('always');
  });

  it('shows progress while fixing, with a cancel that leaves nothing stale on screen', async () => {
    setup();
    let lastCalls = 0;
    api.current.handlers['health.last'] = () => {
      lastCalls += 1;
      return null; // the server forgot the stale result
    };
    api.current.handlers['health.fix'] = (_p, o) => {
      (o as Opts).onProgress?.({ stage: 'health', fraction: 0.4, message: 'Deleting junk files' });
      return pendingUntilAborted(o as Opts);
    };
    renderPage();
    const user = userEvent.setup();
    await scanned(user);
    await user.click(screen.getByTestId('btn-fix-all'));
    await user.click(await screen.findByTestId('confirm-sheet-confirm'));
    const bar = await screen.findByRole('progressbar', { name: 'Fixing progress' });
    expect(bar).toHaveAttribute('aria-valuenow', '40');
    expect(screen.getByTestId('health-progress-message')).toHaveTextContent('Deleting junk files');
    expect(screen.getByTestId('btn-scan')).toBeDisabled();
    expect(screen.queryByTestId('btn-fix-all')).toBeNull();
    await user.click(screen.getByTestId('btn-cancel'));
    expect(await screen.findByTestId('health-notice')).toHaveTextContent('did not finish');
    // the old report described a machine that has changed: it is gone until the next scan
    await waitFor(() => expect(screen.getByTestId('health-status')).toHaveTextContent('Not scanned yet'));
    expect(screen.queryByTestId('btn-fix-all')).toBeNull();
    expect(lastCalls).toBeGreaterThanOrEqual(2);
    expect(screen.getByTestId('btn-scan')).toBeEnabled();
  });

  it('reports a failed fix and forgets the stale report', async () => {
    setup();
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['health.fix'] = () => Promise.reject(new ApiCallError('Io', 'cannot write'));
    renderPage();
    const user = userEvent.setup();
    await scanned(user);
    await user.click(screen.getByTestId('btn-fix-all'));
    await user.click(await screen.findByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('cannot write');
    expect(await screen.findByTestId('health-notice')).toBeInTheDocument();
    expect(screen.queryByTestId('btn-fix-all')).toBeNull();
  });
});

describe('Home: running browsers', () => {
  const blockedFix = (): FixReport => {
    const base = fixReport();
    return {
      ...base,
      parts: [
        { ...base.parts[0]!, status: 'failed', removedRows: 0, blockedApps: ['Google Chrome'] },
        { ...base.parts[1]!, status: 'partial', removedBytes: 1000, removedFiles: 3, blockedApps: ['Google Chrome'] },
        {
          part: 'startup',
          status: 'done',
          message: '',
          removedBytes: 0,
          removedFiles: 0,
          removedRows: 0,
          blockedApps: [],
          items: [{ id: 'xdg:user:slack.desktop', name: 'Slack', ok: true, message: 'Switched off.' }],
        },
      ],
      report: report72,
    };
  };

  it('offers "Close browsers & fix" and retries only the blocked parts', async () => {
    setup();
    const retry: FixReport = {
      ...fixReport(),
      parts: [
        { ...fixReport().parts[0]!, removedRows: 9 },
        { ...fixReport().parts[1]!, removedBytes: 5000, removedFiles: 4 },
      ],
    };
    let n = 0;
    api.current.handlers['health.fix'] = () => (n++ === 0 ? blockedFix() : retry);
    renderPage();
    const user = userEvent.setup();
    await scanned(user);
    await waitFor(() => expect(api.current.paramsOf('settings.get').length).toBeGreaterThan(0));
    await user.click(screen.getByTestId('btn-fix-all'));
    await user.click(await screen.findByTestId('confirm-sheet-confirm'));

    // policy "ask": the app asks at once
    const sheet = await screen.findByTestId('confirm-sheet');
    expect(sheet).toHaveTextContent('Google Chrome is running');
    expect(screen.getByTestId('health-blocked')).toHaveTextContent('Google Chrome is running');
    expect(screen.getByTestId('done-space')).toHaveTextContent('Google Chrome still running');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));

    await waitFor(() => expect(api.current.paramsOf('health.fix')).toHaveLength(2));
    expect(api.current.paramsOf('health.fix')[1]).toEqual({ closeApps: 'always', privacy: true, space: true });
    await waitFor(() => expect(screen.queryByTestId('health-blocked')).toBeNull());
    expect(screen.getByTestId('health-result-title')).toHaveTextContent('All done');
    expect(screen.getByTestId('done-privacy')).toHaveTextContent('Removed 9 browser entries');
    // 1000 B + 5000 B, 3 + 4 files
    expect(screen.getByTestId('done-space')).toHaveTextContent('Deleted 5.9 KB (7 files)');
    // the part that had already been done is still listed, and was not repeated
    expect(screen.getByTestId('done-startup')).toHaveTextContent('Switched off 1 of 1 startup item');
    expect(screen.getByTestId('health-result-score')).toHaveTextContent('Score 72 → 100');
  });

  it('keeps a Close-and-fix button in the result when the user skips the prompt', async () => {
    setup();
    api.current.handlers['health.fix'] = () => blockedFix();
    renderPage();
    const user = userEvent.setup();
    await scanned(user);
    await waitFor(() => expect(api.current.paramsOf('settings.get').length).toBeGreaterThan(0));
    await user.click(screen.getByTestId('btn-fix-all'));
    await user.click(await screen.findByTestId('confirm-sheet-confirm'));
    const sheet = await screen.findByTestId('confirm-sheet');
    await user.click(within(sheet).getByRole('button', { name: 'Skip' }));
    expect(screen.queryByTestId('confirm-sheet')).toBeNull();
    expect(screen.getByTestId('btn-close-and-fix')).toBeEnabled();
    expect(screen.getByTestId('health-result-title')).toHaveTextContent('Done, with some issues');
    expect(api.current.paramsOf('health.fix')).toHaveLength(1);
  });

  it('does not ask again by itself under the "skip" policy', async () => {
    setup({ settings: { closeBrowsers: 'skip' } });
    api.current.handlers['health.fix'] = () => blockedFix();
    renderPage();
    const user = userEvent.setup();
    await scanned(user);
    await waitFor(() => expect(api.current.paramsOf('settings.get').length).toBeGreaterThan(0));
    await user.click(screen.getByTestId('btn-fix-all'));
    await user.click(await screen.findByTestId('confirm-sheet-confirm'));
    await screen.findByTestId('health-result');
    expect(screen.queryByTestId('confirm-sheet')).toBeNull();
    expect(screen.getByTestId('btn-close-and-fix')).toBeInTheDocument();
  });
});
