import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ListPointsResult, OpResult, RestorePoint } from '../api/restore';
import { createApiMock } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import SystemRestorePage from './SystemRestorePage';

const point = (over: Partial<RestorePoint> & { id: string }): RestorePoint => ({
  description: 'A point',
  createdAt: '2024-03-01T10:00:00',
  kind: 'timeshift',
  deletable: true,
  restorable: false,
  isNewest: false,
  ...over,
});

const OLD = point({ id: 'timeshift:2024-01-01_10-00-01', description: 'Old one', note: 'daily' });
const NEWEST = point({
  id: 'timeshift:2024-03-01_03-00-00',
  description: 'Newest',
  isNewest: true,
  deletable: false,
});
const BACKUP = point({
  id: 'clearsweep:config-100',
  description: 'Configuration backup (3 items)',
  kind: 'clearsweep-backup',
  backupKind: 'config',
  restorable: true,
  sizeBytes: 2048,
});

const listing = (over: Partial<ListPointsResult> = {}): ListPointsResult => ({
  os: 'linux',
  supported: true,
  tool: 'timeshift',
  tools: ['timeshift'],
  hint: null,
  canCreate: true,
  canDeleteOld: false,
  canOpenSystemTool: true,
  needsAdmin: false,
  warnings: [],
  points: [NEWEST, OLD, BACKUP],
  ...over,
});

const ok = (message = 'Done.'): OpResult => ({ ok: true, message });

function renderPage() {
  return render(
    <MemoryRouter>
      <SystemRestorePage />
    </MemoryRouter>,
  );
}

let current: ListPointsResult;

beforeEach(() => {
  api.current = createApiMock();
  current = listing();
  api.current.handlers['restore.list_points'] = () => current;
});

describe('listing', () => {
  it('shows kind badges, dates, sizes and protects the most recent point', async () => {
    renderPage();
    await screen.findByTestId('restore-list');
    expect(screen.getByTestId(`point-kind-${OLD.id}`)).toHaveTextContent('Timeshift');
    expect(screen.getByTestId(`point-${OLD.id}`)).toHaveTextContent('Old one');
    expect(screen.getByTestId(`point-${OLD.id}`)).toHaveTextContent('daily');
    expect(screen.getByTestId(`point-newest-${NEWEST.id}`)).toHaveTextContent('Most recent');
    expect(screen.getByTestId(`point-delete-${NEWEST.id}`)).toBeDisabled();
    expect(screen.getByTestId(`point-blocked-${NEWEST.id}`)).toHaveTextContent('most recent');
    expect(screen.getByTestId(`point-delete-${OLD.id}`)).toBeEnabled();
    expect(screen.getByTestId(`point-kind-${BACKUP.id}`)).toHaveTextContent('Configuration backup');
    expect(screen.getByTestId(`point-${BACKUP.id}`)).toHaveTextContent('2.0 KB');
    expect(screen.getByTestId(`point-restore-${BACKUP.id}`)).toBeEnabled();
    expect(screen.queryByTestId(`point-restore-${OLD.id}`)).toBeNull();
  });

  it('explains what to install when there is no tool, and still shows ClearSweep backups', async () => {
    current = listing({
      supported: false,
      tool: null,
      tools: [],
      canCreate: false,
      canOpenSystemTool: false,
      hint: 'No snapshot tool found. Install Timeshift or Snapper.',
      points: [BACKUP],
    });
    renderPage();
    expect(await screen.findByTestId('restore-hint')).toHaveTextContent('Install Timeshift or Snapper');
    expect(screen.queryByTestId('restore-create')).toBeNull();
    expect(screen.queryByTestId('restore-open-tool')).toBeNull();
    expect(screen.getByTestId('restore-backups')).toBeInTheDocument();
  });

  it('asks for authorization when listing needs administrator rights', async () => {
    current = listing({ needsAdmin: true, points: [] });
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('restore-authorize'));
    expect(api.current.paramsOf('restore.list_points')).toEqual([{}, { elevate: true }]);
    expect(screen.queryByTestId('restore-empty')).toBeNull();
  });

  it('shows an empty state and warnings', async () => {
    current = listing({ points: [], warnings: ['snapper said hello'] });
    renderPage();
    expect(await screen.findByTestId('restore-empty')).toHaveTextContent('No restore points yet');
    expect(screen.getByTestId('restore-warning')).toHaveTextContent('snapper said hello');
  });

  it('shows an error banner when listing fails', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['restore.list_points'] = () => Promise.reject(new ApiCallError('Internal', 'boom'));
    renderPage();
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('boom');
  });
});

describe('creating', () => {
  it('validates the description, confirms, creates and reloads', async () => {
    api.current.handlers['restore.create_point'] = () => ok('Restore point created.');
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('restore-create');
    expect(screen.getByTestId('restore-create')).toBeDisabled();
    await user.type(screen.getByTestId('restore-description'), '-bad');
    expect(screen.getByTestId('restore-description-problem')).toHaveTextContent('dash');
    expect(screen.getByTestId('restore-create')).toBeDisabled();
    await user.clear(screen.getByTestId('restore-description'));
    await user.type(screen.getByTestId('restore-description'), '  Before update ');
    await user.click(screen.getByTestId('restore-create'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Before update');
    expect(api.current.paramsOf('restore.create_point')).toEqual([]);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('restore-note')).toHaveTextContent('Restore point created.'));
    expect(api.current.paramsOf('restore.create_point')).toEqual([{ description: 'Before update' }]);
    expect(api.current.paramsOf('restore.list_points').length).toBe(2);
    expect(screen.getByTestId('restore-description')).toHaveValue('');
  });

  it('shows the throttling message from Windows and keeps the description', async () => {
    api.current.handlers['restore.create_point'] = () => ({
      ok: false,
      throttled: true,
      message: 'Windows allows one automatic restore point every 24 hours.',
    });
    renderPage();
    const user = userEvent.setup();
    await user.type(await screen.findByTestId('restore-description'), 'again');
    await user.click(screen.getByTestId('restore-create'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('restore-note')).toHaveTextContent('24 hours'));
    expect(screen.getByTestId('restore-description')).toHaveValue('again');
  });
});

describe('deleting and restoring', () => {
  it('deletes an older point only after confirmation', async () => {
    api.current.handlers['restore.delete_point'] = () => ok('Restore point deleted.');
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`point-delete-${OLD.id}`));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('permanently deleted');
    expect(api.current.paramsOf('restore.delete_point')).toEqual([]);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('restore-note')).toHaveTextContent('deleted'));
    expect(api.current.paramsOf('restore.delete_point')).toEqual([{ id: OLD.id }]);
  });

  it('never sends a delete for the newest point', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`point-delete-${NEWEST.id}`));
    expect(screen.queryByTestId('confirm-sheet')).toBeNull();
    expect(api.current.paramsOf('restore.delete_point')).toEqual([]);
  });

  it('restores a ClearSweep backup after confirmation', async () => {
    api.current.handlers['restore.restore'] = () => ok('Restored 3 items.');
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`point-restore-${BACKUP.id}`));
    expect(api.current.paramsOf('restore.restore')).toEqual([]);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('restore-note')).toHaveTextContent('Restored 3 items.'));
    expect(api.current.paramsOf('restore.restore')).toEqual([{ id: BACKUP.id }]);
  });

  it('surfaces a server refusal', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['restore.delete_point'] = () =>
      Promise.reject(new ApiCallError('InvalidParams', 'The most recent restore point cannot be deleted'));
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`point-delete-${OLD.id}`));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('most recent');
  });
});

describe('Windows extras', () => {
  const win = (id: string, n: number) =>
    point({
      id,
      kind: 'windows-restore-point',
      description: `W${n}`,
      deletable: false,
      isNewest: n === 3,
    });

  it('offers bulk deletion instead of single deletes, and opens the system tool', async () => {
    current = listing({
      os: 'windows',
      tool: 'windows',
      tools: ['windows'],
      canDeleteOld: true,
      points: [win('windows:3', 3), win('windows:2', 2)],
    });
    api.current.handlers['restore.delete_old'] = () => ({ ok: true, message: 'Deleted 1 older restore point.', deleted: 1 });
    api.current.handlers['restore.open_system_tool'] = () => ok('Opened.');
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('restore-list');
    expect(screen.getByTestId('point-delete-windows:2')).toBeDisabled();
    expect(screen.getByTestId('point-blocked-windows:2')).toHaveTextContent('in bulk');
    await user.click(screen.getByTestId('restore-delete-old'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('all but the most recent');
    expect(api.current.paramsOf('restore.delete_old')).toEqual([]);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('restore-note')).toHaveTextContent('Deleted 1 older'));
    await user.click(screen.getByTestId('restore-open-tool'));
    await waitFor(() => expect(api.current.paramsOf('restore.open_system_tool').length).toBe(1));
  });

  it('hides bulk deletion with a single restore point', async () => {
    current = listing({ os: 'windows', canDeleteOld: true, points: [win('windows:3', 3)] });
    renderPage();
    await screen.findByTestId('restore-list');
    expect(screen.queryByTestId('restore-delete-old')).toBeNull();
  });
});
