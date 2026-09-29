import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DupGroup, DupScanResult } from '../api/duplicates';
import { createApiMock, pendingUntilAborted } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import DuplicateFinderPage from './DuplicateFinderPage';

const f = (path: string, bytes: number, modified: number) => ({ path, bytes, modified });
const GROUPS: DupGroup[] = [
  {
    groupId: 'g0',
    key: { bytes: 1000, hash: 'ab' },
    files: [f('/d/a/photo.jpg', 1000, 100), f('/d/b/photo.jpg', 1000, 200), f('/d/c/photo (1).jpg', 1000, 300)],
    wastedBytes: 2000,
  },
  {
    groupId: 'g1',
    key: { bytes: 50, hash: 'cd' },
    files: [f('/d/a/note.txt', 50, 100), f('/d/b/note.txt', 50, 200)],
    wastedBytes: 50,
  },
];

const RESULT: DupScanResult = {
  scanId: 'dup-1',
  groups: GROUPS,
  totalGroups: 2,
  totalFiles: 5,
  wastedBytes: 2050,
  scannedFiles: 40,
  truncatedGroups: false,
  errors: { count: 0, samples: [] },
  durationMs: 120,
};

beforeEach(() => {
  api.current = createApiMock();
  api.current.handlers['duplicates.scan'] = () => RESULT;
});

async function addFolderAndScan(path = '/d') {
  const user = userEvent.setup();
  render(<DuplicateFinderPage />);
  expect(screen.getByTestId('btn-dup-scan')).toBeInTheDocument();
  await user.click(screen.getByTestId('btn-dup-options'));
  await user.type(screen.getByTestId('dup-path-input'), path);
  await user.click(screen.getByTestId('dup-path-add'));
  expect(screen.getByTestId('dup-path-list')).toHaveTextContent(path);
  await user.click(screen.getByTestId('dup-options-done'));
  await user.click(screen.getByTestId('btn-dup-scan'));
  await screen.findByTestId('dup-groups');
  return user;
}

const checks = (groupId: string) => within(screen.getByTestId(`dup-group-${groupId}`)).getAllByTestId('dup-check') as HTMLInputElement[];

describe('DuplicateFinderPage', () => {
  it('asks for a folder first and sends the options that were chosen', async () => {
    const user = userEvent.setup();
    render(<DuplicateFinderPage />);
    await user.click(screen.getByTestId('btn-dup-scan'));
    expect(screen.getByTestId('dup-form-error')).toHaveTextContent('at least one folder');
    expect(api.current.calls).toHaveLength(0);
    // the options sheet opened by itself
    await user.type(screen.getByTestId('dup-path-input'), '/d{Enter}');
    await user.type(screen.getByTestId('dup-exclude-input'), '/d/skip{Enter}');
    await user.click(screen.getByTestId('dup-match-name'));
    await user.type(screen.getByTestId('dup-min'), '1.5');
    await user.click(screen.getByTestId('dup-hidden'));
    await user.click(screen.getByTestId('dup-options-done'));
    await user.click(screen.getByTestId('btn-dup-scan'));
    await screen.findByTestId('dup-groups');
    expect(api.current.paramsOf('duplicates.scan')).toEqual([
      {
        paths: ['/d'],
        excludePaths: ['/d/skip'],
        matchBy: { name: true, size: false, modified: false, content: true },
        minSize: 1572864,
        maxSize: undefined,
        includeHidden: true,
        includeSystem: false,
        skipZeroByte: true,
        followLinks: false,
      },
    ]);
  });

  it('lists groups with their wasted size', async () => {
    await addFolderAndScan();
    expect(screen.getByTestId('dup-summary')).toHaveTextContent('2 groups');
    expect(screen.getByTestId('dup-group-g0-summary')).toHaveTextContent('3 files of 1000 B - wastes 2.0 KB');
    expect(screen.getAllByTestId('dup-file')).toHaveLength(5);
    expect(screen.getByTestId('btn-dup-delete')).toBeDisabled();
  });

  it('never lets the last copy of a group be ticked', async () => {
    const user = await addFolderAndScan();
    const [a, b, c] = checks('g0');
    await user.click(a!);
    await user.click(b!);
    expect(a!.checked && b!.checked).toBe(true);
    expect(c!.disabled).toBe(true);
    await user.click(c!); // ignored
    expect(c!.checked).toBe(false);
    expect(screen.queryByTestId('dup-blocked')).not.toBeInTheDocument();
    expect(screen.getByTestId('btn-dup-delete')).toBeEnabled();
    // the two-file group: one tick, then the other is locked
    const [n1, n2] = checks('g1');
    await user.click(n1!);
    expect(n2!.disabled).toBe(true);
    // unticking frees the lock again
    await user.click(a!);
    expect(c!.disabled).toBe(false);
  });

  it('"keep only this" selects the other copies', async () => {
    const user = await addFolderAndScan();
    const rows = within(screen.getByTestId('dup-group-g0')).getAllByTestId('dup-file');
    await user.click(within(rows[1]!).getByTestId('dup-keep-only'));
    expect(checks('g0').map((c) => c.checked)).toEqual([true, false, true]);
  });

  it('auto select applies the rule from the server and leaves one copy per group', async () => {
    api.current.handlers['duplicates.auto_select'] = () => ({
      groups: [
        { groupId: 'g0', selected: ['/d/a/photo.jpg', '/d/b/photo.jpg'] },
        { groupId: 'g1', selected: ['/d/a/note.txt'] },
      ],
      count: 3,
      bytes: 2050,
    });
    const user = await addFolderAndScan();
    await user.click(screen.getByTestId('btn-dup-auto'));
    await user.click(screen.getByTestId('dup-rule-keep_newest'));
    await waitFor(() => expect(screen.getByTestId('dup-note')).toHaveTextContent('Selected 3 files'));
    expect(api.current.paramsOf('duplicates.auto_select')).toEqual([
      { scanId: 'dup-1', rule: 'keep_newest', folder: undefined },
    ]);
    expect(checks('g0').map((c) => c.checked)).toEqual([true, true, false]);
    expect(checks('g1').map((c) => c.checked)).toEqual([true, false]);
    expect(screen.getByTestId('btn-dup-delete')).toHaveTextContent('Delete selected (3)');
  });

  it('a server-side selection that would remove every copy is discarded', async () => {
    api.current.handlers['duplicates.auto_select'] = () => ({
      groups: [{ groupId: 'g1', selected: ['/d/a/note.txt', '/d/b/note.txt'] }],
      count: 2,
      bytes: 100,
    });
    const user = await addFolderAndScan();
    await user.click(screen.getByTestId('btn-dup-auto'));
    await user.click(screen.getByTestId('dup-rule-keep_oldest'));
    await waitFor(() => expect(screen.getByTestId('dup-note')).toHaveTextContent('did not select anything'));
    expect(screen.getByTestId('btn-dup-delete')).toBeDisabled();
  });

  it('deletes the selection after confirmation and drops resolved groups', async () => {
    api.current.handlers['duplicates.delete'] = (p) => {
      const paths = (p as { paths: string[] }).paths;
      return {
        results: paths.map((path) => ({ path, ok: true, bytes: 1000 })),
        deleted: paths.length,
        freedBytes: 1000 * paths.length,
        remainingGroups: 1,
        remainingFiles: 2,
        wastedBytes: 50,
      };
    };
    const user = await addFolderAndScan();
    const [a, b] = checks('g0');
    await user.click(a!);
    await user.click(b!);
    await user.click(screen.getByTestId('btn-dup-delete'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('At least one copy of each stays');
    expect(api.current.paramsOf('duplicates.delete')).toHaveLength(0);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('dup-note')).toHaveTextContent('Deleted 2 files'));
    expect(api.current.paramsOf('duplicates.delete')).toEqual([
      { scanId: 'dup-1', paths: ['/d/a/photo.jpg', '/d/b/photo.jpg'] },
    ]);
    // g0 has a single file left: it is no longer a duplicate group
    expect(screen.queryByTestId('dup-group-g0')).not.toBeInTheDocument();
    expect(screen.getByTestId('dup-group-g1')).toBeInTheDocument();
    expect(screen.getByTestId('btn-dup-delete')).toBeDisabled();
  });

  it('shows files the server refused to delete and keeps them listed', async () => {
    api.current.handlers['duplicates.delete'] = () => ({
      results: [{ path: '/d/a/note.txt', ok: false, error: 'the file changed since the search; run it again', bytes: 0 }],
      deleted: 0,
      freedBytes: 0,
      remainingGroups: 2,
      remainingFiles: 5,
      wastedBytes: 2050,
    });
    const user = await addFolderAndScan();
    await user.click(checks('g1')[0]!);
    await user.click(screen.getByTestId('btn-dup-delete'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('dup-failures')).toHaveTextContent('changed since the search'));
    expect(screen.getByTestId('dup-group-g1')).toBeInTheDocument();
  });

  it('exports through the server and reports the outcome', async () => {
    api.current.handlers['duplicates.export'] = () => ({ format: 'csv', filename: 'r.csv', text: 'a,b\n' });
    const created = vi.fn(() => 'blob:x');
    Object.assign(URL, { createObjectURL: created, revokeObjectURL: vi.fn() });
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => undefined);
    const user = await addFolderAndScan();
    await user.click(screen.getByTestId('btn-dup-export'));
    await user.click(screen.getByTestId('dup-export-csv'));
    await waitFor(() => expect(screen.getByTestId('dup-note')).toHaveTextContent('Saved r.csv'));
    expect(api.current.paramsOf('duplicates.export')).toEqual([{ scanId: 'dup-1', format: 'csv' }]);
    expect(created).toHaveBeenCalled();
  });

  it('can cancel a running search', async () => {
    api.current.handlers['duplicates.scan'] = (_p, opts) => pendingUntilAborted(opts);
    const user = userEvent.setup();
    render(<DuplicateFinderPage />);
    await user.click(screen.getByTestId('btn-dup-options'));
    await user.type(screen.getByTestId('dup-path-input'), '/d{Enter}');
    await user.click(screen.getByTestId('dup-options-done'));
    await user.click(screen.getByTestId('btn-dup-scan'));
    await user.click(await screen.findByTestId('btn-cancel'));
    await waitFor(() => expect(screen.queryByTestId('dup-progress')).not.toBeInTheDocument());
    expect(screen.queryByTestId('error-banner')).not.toBeInTheDocument();
  });

  it('shows the error banner when the search fails', async () => {
    api.current.handlers['duplicates.scan'] = () => {
      throw Object.assign(new Error('nope'), { code: 'PermissionDenied' });
    };
    const user = userEvent.setup();
    render(<DuplicateFinderPage />);
    await user.click(screen.getByTestId('btn-dup-options'));
    await user.type(screen.getByTestId('dup-path-input'), '/d{Enter}');
    await user.click(screen.getByTestId('dup-options-done'));
    await user.click(screen.getByTestId('btn-dup-scan'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('nope');
  });
});
