import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  BackupInfo,
  CategoriesResult,
  FixResult,
  Issue,
  RestoreOutcome,
  ScanResult,
} from '../api/registry_cleaner';
import { createApiMock } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import RegistryPage from './RegistryPage';

const CATEGORIES: CategoriesResult = {
  platform: 'linux',
  title: 'Config Issues',
  categories: [
    { id: 'desktop_entries', label: 'Broken launchers', description: '', severity: 'low', defaultSelected: true },
    { id: 'broken_symlinks', label: 'Broken links', description: '', severity: 'low', defaultSelected: true },
    { id: 'orphaned_packages', label: 'Orphaned packages', description: '', severity: 'medium', defaultSelected: false },
  ],
};

const issue = (id: string, category: string, extra: Partial<Issue> = {}): Issue => ({
  id,
  category,
  description: `Problem ${id}`,
  location: `/home/u/${id}`,
  severity: 'low',
  needsAdmin: false,
  ...extra,
});

const ISSUES = [
  issue('a1', 'desktop_entries'),
  issue('a2', 'desktop_entries'),
  issue('b1', 'broken_symlinks'),
  issue('c1', 'orphaned_packages', { severity: 'medium', needsAdmin: true }),
];

const scanResult = (issues = ISSUES): ScanResult => ({
  platform: 'linux',
  title: 'Config Issues',
  issues,
  counts: {},
  scanned: ['desktop_entries', 'broken_symlinks', 'orphaned_packages'],
  skipped: [],
});

function renderPage() {
  return render(
    <MemoryRouter>
      <RegistryPage />
    </MemoryRouter>,
  );
}

let backups: BackupInfo[];

beforeEach(() => {
  api.current = createApiMock();
  backups = [];
  api.current.handlers['registry_cleaner.categories'] = () => CATEGORIES;
  api.current.handlers['registry_cleaner.scan'] = () => scanResult();
  api.current.handlers['registry_cleaner.list_backups'] = () => backups;
});

async function scan() {
  const user = userEvent.setup();
  await screen.findByTestId('registry-category-desktop_entries');
  await user.click(screen.getByTestId('registry-scan'));
  await screen.findByTestId('registry-results');
  return user;
}

describe('categories', () => {
  it('ticks the default categories and scans only the chosen ones', async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('registry-category-desktop_entries');
    expect(screen.getByTestId('registry-category-desktop_entries')).toBeChecked();
    expect(screen.getByTestId('registry-category-orphaned_packages')).not.toBeChecked();
    await user.click(screen.getByTestId('registry-category-broken_symlinks'));
    await user.click(screen.getByTestId('registry-scan'));
    await screen.findByTestId('registry-results');
    expect(api.current.paramsOf('registry_cleaner.scan')).toEqual([{ categories: ['desktop_entries'] }]);
  });

  it('All / None and a disabled scan button without categories', async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('registry-category-desktop_entries');
    await user.click(screen.getByTestId('registry-cats-none'));
    expect(screen.getByTestId('registry-scan')).toBeDisabled();
    await user.click(screen.getByTestId('registry-cats-all'));
    expect(screen.getByTestId('registry-category-orphaned_packages')).toBeChecked();
    expect(screen.getByTestId('registry-scan')).toBeEnabled();
  });
});

describe('results', () => {
  it('groups issues, preselects safe categories and counts the selection', async () => {
    renderPage();
    await scan();
    expect(screen.getByTestId('registry-group-count-desktop_entries')).toHaveTextContent('2');
    expect(screen.getByTestId('issue-check-a1')).toBeChecked();
    expect(screen.getByTestId('issue-check-b1')).toBeChecked();
    expect(screen.getByTestId('issue-check-c1')).not.toBeChecked();
    expect(screen.getByTestId('registry-fix')).toHaveTextContent('Fix selected issues (3)');
    expect(within(screen.getByTestId('issue-c1')).getByText('Needs admin')).toBeInTheDocument();
    expect(within(screen.getByTestId('issue-c1')).getByText('Check first')).toBeInTheDocument();
  });

  it('select all, group checkbox and single toggles', async () => {
    renderPage();
    const user = await scan();
    await user.click(screen.getByTestId('registry-select-all'));
    expect(screen.getByTestId('registry-fix')).toHaveTextContent('(4)');
    await user.click(screen.getByTestId('registry-select-all'));
    expect(screen.getByTestId('registry-fix')).toBeDisabled();
    await user.click(screen.getByTestId('registry-group-check-desktop_entries'));
    expect(screen.getByTestId('issue-check-a2')).toBeChecked();
    await user.click(screen.getByTestId('issue-check-a2'));
    expect(screen.getByTestId('registry-fix')).toHaveTextContent('(1)');
  });

  it('shows the empty state', async () => {
    api.current.handlers['registry_cleaner.scan'] = () => scanResult([]);
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('registry-category-desktop_entries');
    await user.click(screen.getByTestId('registry-scan'));
    expect(await screen.findByTestId('registry-empty')).toHaveTextContent('No issues found');
    expect(screen.queryByTestId('registry-fix')).toBeNull();
  });

  it('reports categories that could not be scanned', async () => {
    api.current.handlers['registry_cleaner.scan'] = () => ({
      ...scanResult(),
      skipped: [{ category: 'services', reason: 'access denied' }],
    });
    renderPage();
    await scan();
    expect(screen.getByTestId('registry-skipped')).toHaveTextContent('services (access denied)');
  });
});

describe('fixing', () => {
  it('asks first, mentions the backup, then fixes exactly the ticked ids with backup: true', async () => {
    api.current.handlers['registry_cleaner.fix'] = (): FixResult => ({
      backupId: 'config-100',
      fixed: 3,
      failed: 0,
      results: [
        { id: 'a1', ok: true },
        { id: 'a2', ok: true },
        { id: 'b1', ok: true },
      ],
    });
    renderPage();
    const user = await scan();
    await user.click(screen.getByTestId('registry-fix'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('backup');
    expect(api.current.paramsOf('registry_cleaner.fix')).toEqual([]); // nothing before confirming
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await screen.findByTestId('registry-fix-result');
    expect(api.current.paramsOf('registry_cleaner.fix')).toEqual([
      {
        issueIds: ['a1', 'a2', 'b1'],
        backup: true,
        categories: ['desktop_entries', 'broken_symlinks', 'orphaned_packages'],
      },
    ]);
    expect(screen.getByTestId('registry-fix-summary')).toHaveTextContent('Fixed 3.');
    // fixed items leave the list, the unfixed one stays
    expect(screen.queryByTestId('issue-a1')).toBeNull();
    expect(screen.getByTestId('issue-c1')).toBeInTheDocument();
  });

  it('cancelling the confirmation changes nothing', async () => {
    renderPage();
    const user = await scan();
    await user.click(screen.getByTestId('registry-fix'));
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByTestId('confirm-sheet')).toBeNull();
    expect(api.current.paramsOf('registry_cleaner.fix')).toEqual([]);
  });

  it('lists failures and offers to restore the backup', async () => {
    api.current.handlers['registry_cleaner.fix'] = (): FixResult => ({
      backupId: 'config-100',
      fixed: 2,
      failed: 1,
      results: [
        { id: 'a1', ok: true },
        { id: 'a2', ok: false, error: 'permission denied' },
        { id: 'b1', ok: true },
      ],
    });
    const restored: RestoreOutcome = { id: 'config-100', ok: true, restored: 3, failed: 0, results: [] };
    api.current.handlers['registry_cleaner.restore_backup'] = () => restored;
    renderPage();
    const user = await scan();
    await user.click(screen.getByTestId('registry-fix'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await screen.findByTestId('registry-fix-result');
    expect(screen.getByTestId('registry-fix-failures')).toHaveTextContent('Problem a2: permission denied');
    expect(screen.getByTestId('registry-fix-summary')).toHaveTextContent('1 could not be fixed');
    await user.click(screen.getByTestId('registry-restore-backup'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Restore the backup?');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('registry-note')).toHaveTextContent('Restored 3 items'));
    expect(api.current.paramsOf('registry_cleaner.restore_backup')).toEqual([{ id: 'config-100' }]);
    expect(screen.queryByTestId('registry-fix-result')).toBeNull();
  });

  it('shows the error when fixing fails (for example the backup could not be made)', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['registry_cleaner.fix'] = () =>
      Promise.reject(new ApiCallError('Io', 'Could not back up first, so nothing was changed.'));
    renderPage();
    const user = await scan();
    await user.click(screen.getByTestId('registry-fix'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('nothing was changed');
    expect(screen.queryByTestId('registry-fix-result')).toBeNull();
    // the list is still there so the user can retry
    expect(screen.getByTestId('issue-a1')).toBeInTheDocument();
  });
});

describe('backups sub-page', () => {
  it('lists, restores after confirmation and deletes after confirmation', async () => {
    backups = [
      { id: 'config-200', createdAt: '2024-05-01T10:00:00Z', issueCount: 3, sizeBytes: 2048, platform: 'linux', kind: 'config' },
      { id: 'config-100', createdAt: '2024-04-01T10:00:00Z', issueCount: 1, sizeBytes: 100, platform: 'linux', kind: 'config' },
    ];
    api.current.handlers['registry_cleaner.restore_backup'] = () =>
      ({ id: 'config-200', ok: true, restored: 3, failed: 0, results: [] }) satisfies RestoreOutcome;
    api.current.handlers['registry_cleaner.delete_backup'] = () => {
      backups = backups.filter((b) => b.id !== 'config-100');
      return { id: 'config-100', freedBytes: 100 };
    };
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('registry-open-backups'));
    await screen.findByTestId('backups-list');
    expect(screen.getByTestId('backup-row-config-200')).toHaveTextContent('3 items');
    await user.click(screen.getByTestId('backup-restore-config-200'));
    expect(api.current.paramsOf('registry_cleaner.restore_backup')).toEqual([]);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('backups-note')).toHaveTextContent('Restored 3 items'));
    await user.click(screen.getByTestId('backup-delete-config-100'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('permanently deleted');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.queryByTestId('backup-row-config-100')).toBeNull());
    expect(api.current.paramsOf('registry_cleaner.delete_backup')).toEqual([{ id: 'config-100' }]);
    await user.click(screen.getByTestId('backups-back'));
    expect(await screen.findByTestId('registry-scan')).toBeInTheDocument();
  });

  it('shows the empty state', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('registry-open-backups'));
    expect(await screen.findByTestId('backups-empty')).toHaveTextContent('No backups yet');
  });
});
