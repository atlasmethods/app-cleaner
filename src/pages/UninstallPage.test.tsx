import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppEntry, RunResult } from '../api/uninstall';
import { createApiMock, pendingUntilAborted } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import UninstallPage from './UninstallPage';

const app = (over: Partial<AppEntry> & { id: string; name: string }): AppEntry => ({
  version: '1.0',
  publisher: 'Acme',
  source: 'dpkg',
  uninstallable: true,
  canRepair: false,
  canModify: false,
  isSystem: false,
  ...over,
});

const FIREFOX = app({ id: 'dpkg:firefox', name: 'firefox', version: '126.0', publisher: 'Mozilla', sizeBytes: 300 * 1024 * 1024 });
const GIMP = app({ id: 'dpkg:gimp', name: 'gimp', sizeBytes: 30 * 1024 * 1024, installDate: '2024-02-01' });
const BASH = app({ id: 'dpkg:bash', name: 'bash', isSystem: true, sizeBytes: 2000 });
const WIN = app({
  id: 'windows:hklm64:{G}',
  name: 'Office',
  source: 'windows',
  canRepair: true,
  canModify: true,
});
const LOCKED = app({ id: 'windows:hklm64:Locked', name: 'Locked', source: 'windows', uninstallable: false });

const result = (over: Partial<RunResult> = {}): RunResult => ({
  id: 'dpkg:gimp',
  name: 'gimp',
  ok: true,
  exitCode: 0,
  message: 'apt-get remove finished',
  alsoRemoves: [],
  needsForce: false,
  rebootRequired: false,
  leftovers: [],
  ...over,
});

let apps: AppEntry[];

function renderPage() {
  return render(
    <MemoryRouter>
      <UninstallPage />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  api.current = createApiMock();
  apps = [FIREFOX, GIMP, BASH, WIN, LOCKED];
  api.current.handlers['uninstall.list'] = () => apps;
});

const rowNames = () => screen.queryAllByTestId('app-name').map((e) => e.textContent);

describe('UninstallPage list', () => {
  it('lists programs, hides system components by default and can show them', async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('uninstall-list');
    expect(rowNames()).toEqual(['firefox', 'gimp', 'Locked', 'Office']);
    expect(screen.getByTestId('app-row-dpkg:firefox')).toHaveTextContent('126.0 - Mozilla');
    expect(screen.getByTestId('app-row-dpkg:firefox')).toHaveTextContent('300 MB');
    expect(screen.getByTestId('uninstall-list')).toHaveTextContent('Installed programs (4)');
    expect(screen.getByText(/Show system components \(1\)/)).toBeInTheDocument();
    await user.click(screen.getByTestId('uninstall-show-system'));
    expect(rowNames()).toEqual(['bash', 'firefox', 'gimp', 'Locked', 'Office']);
  });

  it('searches and sorts', async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('uninstall-list');
    await user.type(screen.getByTestId('uninstall-search'), 'moz');
    expect(rowNames()).toEqual(['firefox']);
    await user.clear(screen.getByTestId('uninstall-search'));
    await user.selectOptions(screen.getByTestId('uninstall-sort'), 'size');
    expect(rowNames()[0]).toBe('firefox');
    await user.selectOptions(screen.getByTestId('uninstall-sort'), 'date');
    expect(rowNames()[0]).toBe('gimp');
    await user.type(screen.getByTestId('uninstall-search'), 'zzz');
    expect(screen.getByTestId('uninstall-empty')).toHaveTextContent('Nothing matches');
  });

  it('shows an error banner with retry when listing fails', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['uninstall.list'] = () => Promise.reject(new ApiCallError('Internal', 'boom'));
    renderPage();
    const user = userEvent.setup();
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('boom');
    api.current.handlers['uninstall.list'] = () => apps;
    await user.click(within(screen.getByTestId('error-banner')).getByRole('button', { name: 'Retry' }));
    await screen.findByTestId('uninstall-list');
    expect(screen.queryByTestId('error-banner')).toBeNull();
  });

  it('shows the empty state', async () => {
    apps = [];
    renderPage();
    expect(await screen.findByTestId('uninstall-empty')).toHaveTextContent('No programs found');
  });
});

describe('row menu', () => {
  it('offers only Uninstall for a Linux package', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('app-row-dpkg:firefox'));
    expect(screen.getByTestId('app-sheet')).toBeInTheDocument();
    expect(screen.getByTestId('app-action-uninstall')).toBeInTheDocument();
    expect(screen.queryByTestId('app-action-repair')).toBeNull();
    expect(screen.queryByTestId('app-action-rename')).toBeNull();
    expect(screen.queryByTestId('app-action-remove-entry')).toBeNull();
  });

  it('offers repair, rename and delete entry for Windows programs, and hides uninstall without an uninstaller', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('app-row-windows:hklm64:{G}'));
    for (const t of ['app-action-uninstall', 'app-action-repair', 'app-action-rename', 'app-action-remove-entry']) {
      expect(screen.getByTestId(t)).toBeInTheDocument();
    }
    await user.click(screen.getByTestId('app-sheet-close'));
    expect(screen.queryByTestId('app-sheet')).toBeNull();
    await user.click(screen.getByTestId('app-row-windows:hklm64:Locked'));
    expect(screen.queryByTestId('app-action-uninstall')).toBeNull();
    expect(screen.getByTestId('app-sheet-no-uninstaller')).toBeInTheDocument();
    expect(screen.getByTestId('app-action-remove-entry')).toBeInTheDocument();
  });
});

describe('uninstall flow', () => {
  it('asks for confirmation, runs, then offers leftovers and removes the ticked ones', async () => {
    const left = [
      { path: '/home/u/.config/gimp', sizeBytes: 4096, kind: 'config' as const },
      { path: '/home/u/.cache/gimp', sizeBytes: 2048, kind: 'cache' as const },
    ];
    api.current.handlers['uninstall.run'] = () => {
      apps = apps.filter((a) => a.id !== 'dpkg:gimp');
      return result({ leftovers: left });
    };
    api.current.handlers['uninstall.remove_leftovers'] = () => ({
      results: [{ path: left[0]!.path, ok: true, bytes: 4096 }],
      totalBytes: 4096,
    });
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('app-row-dpkg:gimp'));
    await user.click(screen.getByTestId('app-action-uninstall'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Uninstall gimp?');
    expect(api.current.paramsOf('uninstall.run')).toHaveLength(0); // nothing ran yet
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('uninstall.run')).toEqual([{ id: 'dpkg:gimp' }]));
    expect(await screen.findByTestId('uninstall-note')).toHaveTextContent('gimp: apt-get remove finished.');
    // list reloaded without gimp
    await waitFor(() => expect(rowNames()).not.toContain('gimp'));
    const card = await screen.findByTestId('leftovers-card');
    expect(card).toHaveTextContent('/home/u/.config/gimp');
    expect(card).toHaveTextContent('Remove (6.0 KB)');
    // untick the cache dir, then remove
    await user.click(screen.getByTestId('leftover-check-1'));
    expect(screen.getByTestId('btn-remove-leftovers')).toHaveTextContent('Remove (4.0 KB)');
    await user.click(screen.getByTestId('btn-remove-leftovers'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Remove leftovers?');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('uninstall.remove_leftovers')).toHaveLength(1));
    expect(api.current.paramsOf('uninstall.remove_leftovers')[0]).toEqual({
      name: 'gimp',
      id: 'dpkg:gimp',
      paths: ['/home/u/.config/gimp'],
    });
    await waitFor(() => expect(screen.queryByTestId('leftovers-card')).toBeNull());
    expect(screen.getByTestId('uninstall-note')).toHaveTextContent('Removed 1 of 1 leftover items (4.0 KB freed)');
  });

  it('"Keep" dismisses the leftovers without deleting anything', async () => {
    api.current.handlers['uninstall.run'] = () =>
      result({ leftovers: [{ path: '/home/u/.config/gimp', sizeBytes: 1, kind: 'config' }] });
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('app-row-dpkg:gimp'));
    await user.click(screen.getByTestId('app-action-uninstall'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await screen.findByTestId('leftovers-card');
    await user.click(screen.getByTestId('btn-leftovers-dismiss'));
    expect(screen.queryByTestId('leftovers-card')).toBeNull();
    expect(api.current.paramsOf('uninstall.remove_leftovers')).toHaveLength(0);
  });

  it('cancelling the confirmation runs nothing', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('app-row-dpkg:gimp'));
    await user.click(screen.getByTestId('app-action-uninstall'));
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByTestId('confirm-sheet')).toBeNull();
    expect(api.current.paramsOf('uninstall.run')).toHaveLength(0);
  });

  it('a failed uninstall shows the reason and no leftovers', async () => {
    api.current.handlers['uninstall.run'] = () =>
      result({ ok: false, exitCode: 100, message: 'apt-get remove failed (exit code 100): E: Could not get lock' });
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('app-row-dpkg:gimp'));
    await user.click(screen.getByTestId('app-action-uninstall'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('uninstall-note')).toHaveTextContent('Could not get lock');
    expect(screen.queryByTestId('leftovers-card')).toBeNull();
  });

  it('asks again when other packages would be removed, then forces', async () => {
    let n = 0;
    api.current.handlers['uninstall.run'] = () => {
      n += 1;
      return n === 1
        ? result({ ok: false, needsForce: true, alsoRemoves: ['gimp-plugin', 'gimp-data'], message: 'would remove others' })
        : result();
    };
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('app-row-dpkg:gimp'));
    await user.click(screen.getByTestId('app-action-uninstall'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Also remove 2 other packages?'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('gimp-plugin, gimp-data');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('uninstall.run')).toHaveLength(2));
    expect(api.current.paramsOf('uninstall.run')).toEqual([{ id: 'dpkg:gimp' }, { id: 'dpkg:gimp', force: true }]);
  });

  it('system components need a stronger warning and are sent with force', async () => {
    api.current.handlers['uninstall.run'] = () => result({ id: 'dpkg:bash', name: 'bash' });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('uninstall-list');
    await user.click(screen.getByTestId('uninstall-show-system'));
    await user.click(screen.getByTestId('app-row-dpkg:bash'));
    expect(screen.getByTestId('app-sheet-system')).toBeInTheDocument();
    await user.click(screen.getByTestId('app-action-uninstall'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('system component');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('uninstall.run')).toEqual([{ id: 'dpkg:bash', force: true }]));
  });

  it('shows progress and can be cancelled', async () => {
    api.current.handlers['uninstall.run'] = (_p, opts) => {
      opts.onProgress?.({ stage: 'uninstall', message: 'Uninstalling gimp', fraction: 0.4 });
      return pendingUntilAborted(opts);
    };
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('app-row-dpkg:gimp'));
    await user.click(screen.getByTestId('app-action-uninstall'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('uninstall-progress-message')).toHaveTextContent('Uninstalling gimp');
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '40');
    await user.click(screen.getByTestId('btn-cancel'));
    await waitFor(() => expect(screen.queryByTestId('uninstall-progress')).toBeNull());
    expect(screen.queryByTestId('error-banner')).toBeNull();
  });

  it('shows an API error from the run', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['uninstall.run'] = () =>
      Promise.reject(new ApiCallError('PermissionDenied', 'Authorization was cancelled'));
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('app-row-dpkg:gimp'));
    await user.click(screen.getByTestId('app-action-uninstall'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('Authorization was cancelled');
  });
});

describe('Windows entry actions', () => {
  it('repair, delete entry and rename go through confirmations and the matching methods', async () => {
    api.current.handlers['uninstall.repair'] = () => result({ id: 'windows:hklm64:{G}', name: 'Office', message: 'The repair finished' });
    api.current.handlers['uninstall.remove_entry'] = () =>
      result({ id: 'windows:hklm64:{G}', name: 'Office', message: 'reg delete finished', backupPath: 'C:\\data\\backups\\uninstall-1.reg' });
    api.current.handlers['uninstall.rename_entry'] = () => result({ id: 'windows:hklm64:{G}', name: 'Office', message: 'reg add finished', newName: 'Office 2010' });
    renderPage();
    const user = userEvent.setup();

    await user.click(await screen.findByTestId('app-row-windows:hklm64:{G}'));
    await user.click(screen.getByTestId('app-action-repair'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Repair Office?');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('uninstall.repair')).toEqual([{ id: 'windows:hklm64:{G}' }]));

    await user.click(await screen.findByTestId('app-row-windows:hklm64:{G}'));
    await user.click(screen.getByTestId('app-action-remove-entry'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Delete the entry for Office?');
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('backup');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('uninstall.remove_entry')).toHaveLength(1));
    expect(await screen.findByTestId('uninstall-note')).toHaveTextContent('Backup: C:\\data\\backups\\uninstall-1.reg');

    await user.click(await screen.findByTestId('app-row-windows:hklm64:{G}'));
    await user.click(screen.getByTestId('app-action-rename'));
    const input = screen.getByTestId('app-rename-input');
    expect(input).toHaveValue('Office');
    await user.clear(input);
    await user.type(input, 'Office 2010');
    await user.click(screen.getByTestId('app-rename-continue'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Office 2010');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() =>
      expect(api.current.paramsOf('uninstall.rename_entry')).toEqual([{ id: 'windows:hklm64:{G}', name: 'Office 2010' }]),
    );
  });

  it('reopening the menu after a rename starts on the action list again', async () => {
    api.current.handlers['uninstall.rename_entry'] = () => result();
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('app-row-windows:hklm64:{G}'));
    await user.click(screen.getByTestId('app-action-rename'));
    await user.click(screen.getByTestId('app-rename-continue'));
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    await user.click(screen.getByTestId('app-row-windows:hklm64:{G}'));
    expect(screen.getByTestId('app-action-uninstall')).toBeInTheDocument();
    expect(screen.queryByTestId('app-rename-input')).toBeNull();
  });
});
