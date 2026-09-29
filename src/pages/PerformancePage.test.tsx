import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { OptimizerAnalysis, OptimizerApp } from '../api/optimizer';
import { createApiMock } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import PerformancePage from './PerformancePage';

const MB = 1024 * 1024;
const app = (over: Partial<OptimizerApp> & { appId: string }): OptimizerApp => ({
  name: over.appId,
  processes: [],
  startupIds: [],
  serviceIds: [],
  backgroundMemoryBytes: 0,
  cpuPercent: 0,
  sleeping: false,
  protected: false,
  ...over,
});
const proc = (pid: number, mb: number) => ({ pid, name: 'p', memoryBytes: mb * MB, cpuPercent: 2 });

let apps: OptimizerApp[];

const analysis = (): OptimizerAnalysis => ({
  apps,
  totals: { apps: apps.length, runningApps: 0, backgroundMemoryBytes: 0, sleepingApps: 0, startupItems: 0 },
});

function renderPage() {
  return render(
    <MemoryRouter>
      <PerformancePage />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  api.current = createApiMock();
  apps = [
    app({ appId: 'slack', name: 'Slack', processes: [proc(1, 300)], backgroundMemoryBytes: 300 * MB, cpuPercent: 2, startupIds: ['x'] }),
    app({ appId: 'zoom', name: 'Zoom', processes: [proc(2, 100)], backgroundMemoryBytes: 100 * MB }),
    app({ appId: 'av', name: 'Antivirus', processes: [proc(3, 50)], backgroundMemoryBytes: 50 * MB, protected: true }),
    app({ appId: 'dropbox', name: 'Dropbox', sleeping: true }),
  ];
  api.current.handlers['optimizer.analyze'] = analysis;
});

describe('PerformancePage', () => {
  it('summarises the running apps and lists them biggest first, with sleeping ones apart', async () => {
    renderPage();
    await screen.findByTestId('perf-list');
    expect(screen.getByTestId('perf-summary-count')).toHaveTextContent('3');
    expect(screen.getByTestId('perf-summary-memory')).toHaveTextContent('450 MB');
    expect(screen.getAllByTestId('perf-name').map((e) => e.textContent)).toEqual(['Slack', 'Zoom', 'Antivirus']);
    expect(screen.getByTestId('perf-row-slack')).toHaveTextContent('300 MB - 2.0% CPU - 1 process - 1 startup item');
    expect(within(screen.getByTestId('perf-sleeping')).getByText('Dropbox')).toBeInTheDocument();
    expect(screen.getByTestId('perf-explain')).toHaveTextContent('never force-closed');
  });

  it('protected apps have a disabled toggle and are left out of Sleep all', async () => {
    renderPage();
    await screen.findByTestId('perf-list');
    expect(screen.getByTestId('perf-sleep-av')).toBeDisabled();
    expect(screen.getByTestId('perf-row-av')).toHaveTextContent('Protected');
    expect(screen.getByTestId('perf-sleep-all')).toHaveTextContent('Sleep all (2)');
  });

  it('confirms before sleeping one app, then reports and reloads', async () => {
    api.current.handlers['optimizer.sleep'] = () => {
      apps = apps.map((a) => (a.appId === 'slack' ? { ...a, sleeping: true, processes: [], backgroundMemoryBytes: 0 } : a));
      return { results: [{ appId: 'slack', name: 'Slack', ok: true, stoppedProcesses: 1, stillRunning: 0 }] };
    };
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('perf-list');
    await user.click(screen.getByTestId('perf-sleep-slack'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Put Slack to sleep?');
    expect(api.current.paramsOf('optimizer.sleep')).toEqual([]);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('perf-note')).toHaveTextContent('1 app put to sleep, 1 process closed.'));
    expect(api.current.paramsOf('optimizer.sleep')).toEqual([{ appIds: ['slack'] }]);
    await waitFor(() => expect(screen.getByTestId('perf-sleeping-slack')).toBeInTheDocument());
    expect(screen.queryByTestId('perf-row-slack')).toBeNull();
  });

  it('Sleep all sends every sleepable app after a confirmation', async () => {
    api.current.handlers['optimizer.sleep'] = () => ({ results: [] });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('perf-list');
    await user.click(screen.getByTestId('perf-sleep-all'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Slack, Zoom');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('optimizer.sleep')).toEqual([{ appIds: ['slack', 'zoom'] }]));
  });

  it('wakes a sleeping app without a confirmation', async () => {
    api.current.handlers['optimizer.wake'] = () => {
      apps = apps.map((a) => (a.appId === 'dropbox' ? { ...a, sleeping: false } : a));
      return { results: [{ appId: 'dropbox', name: 'Dropbox', ok: true, restoredItems: ['x'] }] };
    };
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('perf-sleeping');
    await user.click(screen.getByTestId('perf-wake-dropbox'));
    await waitFor(() => expect(screen.getByTestId('perf-note')).toHaveTextContent('1 app woken'));
    expect(api.current.paramsOf('optimizer.wake')).toEqual([{ appIds: ['dropbox'] }]);
    await waitFor(() => expect(screen.queryByTestId('perf-sleeping')).toBeNull());
  });

  it('warns when processes did not quit and when the state file is damaged', async () => {
    api.current.handlers['optimizer.analyze'] = () => ({ ...analysis(), stateError: 'optimizer.json is damaged' });
    api.current.handlers['optimizer.sleep'] = () => ({
      results: [{ appId: 'zoom', name: 'Zoom', ok: true, stoppedProcesses: 0, stillRunning: 1 }],
    });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('perf-state-error');
    await user.click(screen.getByTestId('perf-sleep-zoom'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('perf-note')).toHaveTextContent('still running'));
  });

  it('shows an error banner when analysis fails', async () => {
    api.current.handlers['optimizer.analyze'] = () => {
      throw Object.assign(new Error('x'), { code: 'Io', message: 'cannot read processes' });
    };
    renderPage();
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('cannot read processes');
  });

  it('shows an empty state', async () => {
    apps = [];
    renderPage();
    expect(await screen.findByTestId('perf-empty')).toHaveTextContent('Nothing to put to sleep');
    expect(screen.getByTestId('perf-sleep-all')).toBeDisabled();
  });
});
