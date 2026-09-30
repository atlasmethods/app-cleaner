import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { UpdateEntry, UpdateReport } from '../api/software_updater';
import { createApiMock, pendingUntilAborted } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import SoftwareUpdaterPage from './SoftwareUpdaterPage';

const upd = (over: Partial<UpdateEntry> & { id: string; name: string }): UpdateEntry => ({
  currentVersion: '1.0',
  newVersion: '2.0',
  source: 'apt',
  ignored: false,
  ...over,
});

const FIREFOX = upd({ id: 'apt:firefox', name: 'firefox', currentVersion: '125.0', newVersion: '126.0', security: true });
const VIM = upd({ id: 'apt:vim', name: 'vim' });
const VLC = upd({ id: 'snap:vlc', name: 'vlc', source: 'snap', currentVersion: '', newVersion: '3.0.21' });
const OLD = upd({ id: 'apt:old', name: 'old-thing', ignored: true });

let entries: UpdateEntry[];

function renderPage() {
  return render(
    <MemoryRouter>
      <SoftwareUpdaterPage />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  api.current = createApiMock();
  entries = [FIREFOX, OLD, VIM, VLC];
  api.current.handlers['software_updater.list'] = () => entries;
});

describe('SoftwareUpdaterPage', () => {
  it('lists updates with old -> new versions, source and security badges, ignored dimmed', async () => {
    renderPage();
    await screen.findByTestId('updater-list');
    expect(screen.getByTestId('updater-summary')).toHaveTextContent('4 updates available');
    const ff = screen.getByTestId('update-apt:firefox');
    expect(within(ff).getByTestId('update-versions')).toHaveTextContent('125.0 → 126.0');
    expect(within(ff).getByTestId('update-source')).toHaveTextContent('apt');
    expect(within(ff).getByTestId('update-security')).toBeInTheDocument();
    expect(within(screen.getByTestId('update-snap:vlc')).getByTestId('update-versions')).toHaveTextContent('→ 3.0.21');
    expect(within(screen.getByTestId('update-snap:vlc')).getByTestId('update-source')).toHaveTextContent('Snap');
    expect(within(screen.getByTestId('update-apt:old')).getByTestId('update-ignored')).toBeInTheDocument();
    expect(screen.getByTestId('update-select-apt:old')).toBeDisabled();
    // initial list is loaded without a refresh
    expect(api.current.paramsOf('software_updater.list')).toEqual([{}]);
  });

  it('Refresh reloads the package index first', async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('updater-list');
    await user.click(screen.getByTestId('btn-refresh'));
    await waitFor(() => expect(api.current.paramsOf('software_updater.list')).toHaveLength(2));
    expect(api.current.paramsOf('software_updater.list')[1]).toEqual({ refresh: true });
  });

  it('shows the up-to-date state', async () => {
    entries = [];
    renderPage();
    expect(await screen.findByText('Up to date')).toBeInTheDocument();
    expect(screen.getByTestId('updater-summary')).toHaveTextContent('Everything is up to date.');
    expect(screen.getByTestId('btn-update-all')).toBeDisabled();
  });

  it('Update selected asks for confirmation and sends only the ticked, non-ignored ids', async () => {
    const report: UpdateReport = {
      results: [
        { id: 'apt:firefox', name: 'firefox', ok: true, exitCode: 0, message: 'Updated' },
        { id: 'snap:vlc', name: 'vlc', ok: false, exitCode: 1, message: 'snap refresh failed (exit code 1): error' },
      ],
      succeeded: 1,
      failed: 1,
    };
    api.current.handlers['software_updater.update'] = () => {
      entries = [VLC]; // firefox is gone from the list, vlc still pending
      return report;
    };
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('updater-list');
    expect(screen.getByTestId('btn-update-selected')).toBeDisabled();
    await user.click(screen.getByTestId('update-select-apt:firefox'));
    await user.click(screen.getByTestId('update-select-snap:vlc'));
    expect(screen.getByTestId('btn-update-selected')).toHaveTextContent('Update selected (2)');
    await user.click(screen.getByTestId('btn-update-selected'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Update 2 selected programs?');
    expect(api.current.paramsOf('software_updater.update')).toHaveLength(0);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('software_updater.update')).toEqual([{ ids: ['apt:firefox', 'snap:vlc'] }]));
    // per-item results
    const rep = await screen.findByTestId('updater-report');
    expect(rep).toHaveTextContent('1 succeeded, 1 failed');
    expect(within(rep).getByTestId('report-snap:vlc')).toHaveTextContent('snap refresh failed (exit code 1)');
    // the failed item stays in the list with its status
    await waitFor(() => expect(screen.queryByTestId('update-apt:firefox')).toBeNull());
    expect(screen.getByTestId('update-result-snap:vlc')).toHaveTextContent('Failed: snap refresh failed');
    // list reloaded after the update
    expect(api.current.paramsOf('software_updater.list').length).toBeGreaterThanOrEqual(2);
  });

  it('select all skips ignored entries', async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('updater-list');
    await user.click(screen.getByTestId('updater-select-all'));
    expect(screen.getByTestId('btn-update-selected')).toHaveTextContent('Update selected (3)');
    expect(screen.getByTestId('update-select-apt:old')).not.toBeChecked();
    await user.click(screen.getByTestId('updater-select-all'));
    expect(screen.getByTestId('btn-update-selected')).toHaveTextContent('Update selected (0)');
  });

  it('Update all confirms and calls update_all', async () => {
    api.current.handlers['software_updater.update_all'] = () => ({ results: [], succeeded: 0, failed: 0 });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('updater-list');
    await user.click(screen.getByTestId('btn-update-all'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Update all 3 programs?');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('software_updater.update_all')).toHaveLength(1));
  });

  it('ignore toggle lives in the row menu, persists, and unticks the row', async () => {
    api.current.handlers['software_updater.set_ignored'] = (p) => ({ ...(p as object), ignoredUpdates: ['apt:vim'] });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('updater-list');
    await user.click(screen.getByTestId('update-select-apt:vim'));
    await user.click(screen.getByTestId('update-menu-apt:vim'));
    expect(screen.getByTestId('update-toggle-ignore')).toHaveTextContent('Ignore this update');
    await user.click(screen.getByTestId('update-toggle-ignore'));
    await waitFor(() => expect(api.current.paramsOf('software_updater.set_ignored')).toEqual([{ id: 'apt:vim', ignored: true }]));
    await waitFor(() => expect(screen.getByTestId('update-select-apt:vim')).toBeDisabled());
    expect(screen.getByTestId('btn-update-selected')).toHaveTextContent('Update selected (0)');
    // and back
    await user.click(screen.getByTestId('update-menu-apt:vim'));
    expect(screen.getByTestId('update-toggle-ignore')).toHaveTextContent('Stop ignoring this update');
    await user.click(screen.getByTestId('update-toggle-ignore'));
    await waitFor(() => expect(screen.getByTestId('update-select-apt:vim')).toBeEnabled());
    expect(api.current.paramsOf('software_updater.set_ignored')[1]).toEqual({ id: 'apt:vim', ignored: false });
  });

  it('shows progress with cancel while checking', async () => {
    api.current.handlers['software_updater.list'] = (_p, opts) => {
      opts.onProgress?.({ stage: 'check', message: 'Checking apt', fraction: 0.5 });
      return pendingUntilAborted(opts);
    };
    renderPage();
    const user = userEvent.setup();
    expect(await screen.findByTestId('updater-progress-message')).toHaveTextContent('Checking apt');
    await user.click(screen.getByTestId('btn-cancel'));
    await waitFor(() => expect(screen.queryByTestId('updater-progress')).toBeNull());
    expect(screen.queryByTestId('error-banner')).toBeNull();
  });

  it('shows errors from the list and from updates', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['software_updater.list'] = () =>
      Promise.reject(new ApiCallError('Unsupported', 'No supported package manager was found'));
    renderPage();
    expect(await screen.findByTestId('updater-unsupported')).toHaveTextContent('No supported package manager was found');
    expect(screen.getByTestId('updater-unsupported')).toHaveTextContent("Software updates aren't available on this system");
    expect(screen.queryByTestId('error-banner')).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('a real failure of the list still shows the error banner', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['software_updater.list'] = () => Promise.reject(new ApiCallError('Io', 'apt update failed'));
    renderPage();
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('apt update failed');
    expect(screen.queryByTestId('updater-unsupported')).toBeNull();
  });

  it('update error (authorization cancelled) is shown', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['software_updater.update'] = () =>
      Promise.reject(new ApiCallError('PermissionDenied', 'Authorization was cancelled'));
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('updater-list');
    await user.click(screen.getByTestId('update-select-apt:vim'));
    await user.click(screen.getByTestId('btn-update-selected'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('Authorization was cancelled');
  });
});
