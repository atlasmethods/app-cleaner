import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  CategoryTotals,
  FileRow,
  FilesPage,
  ScanSummary,
  TreeNode,
} from '../api/disk_analyzer';
import { createApiMock, pendingUntilAborted } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import DiskAnalyzerPage from './DiskAnalyzerPage';

const empty = () => ({ files: 0, bytes: 0 });
const TOTALS: CategoryTotals = {
  pictures: { files: 2, bytes: 300 },
  music: { files: 1, bytes: 1000 },
  documents: empty(),
  video: { files: 1, bytes: 5000 },
  compressed: empty(),
  email: empty(),
  other: { files: 1, bytes: 7 },
};

const SUMMARY: ScanSummary = {
  scanId: 'da-1',
  roots: ['/data'],
  totals: TOTALS,
  totalFiles: 5,
  totalBytes: 6307,
  stored: 5,
  truncated: false,
  errors: { count: 0, samples: [] },
  durationMs: 250,
};

const row = (name: string, bytes: number, category: FileRow['category'] = 'pictures'): FileRow => ({
  path: `/data/${name}`,
  name,
  bytes,
  modified: 1_700_000_000,
  category,
});

function setup(files: FileRow[] = [row('b.png', 200), row('a.jpg', 100)]) {
  api.current.handlers['disk_analyzer.list_drives'] = () => [
    { name: 'sda1', mount: '/data', fs: 'ext4', total: 10_000, available: 4_000, removable: false },
  ];
  api.current.handlers['disk_analyzer.scan'] = () => SUMMARY;
  api.current.handlers['disk_analyzer.files'] = (p) => {
    const q = p as { category?: string; sort?: string; offset?: number };
    const page: FilesPage = {
      total: files.length,
      totalBytes: files.reduce((s, r) => s + r.bytes, 0),
      offset: q.offset ?? 0,
      limit: 100,
      truncated: false,
      files: (q.offset ?? 0) === 0 ? files : [],
    };
    return page;
  };
}

async function analyze() {
  const user = userEvent.setup();
  render(<DiskAnalyzerPage />);
  await user.click(await screen.findByTestId('disk-drive-/data'));
  await user.click(screen.getByTestId('btn-disk-analyze'));
  await screen.findByTestId('disk-categories');
  return user;
}

beforeEach(() => {
  api.current = createApiMock();
});

describe('DiskAnalyzerPage', () => {
  it('needs a drive or folder before it can analyze', async () => {
    setup();
    const user = userEvent.setup();
    render(<DiskAnalyzerPage />);
    const btn = screen.getByTestId('btn-disk-analyze');
    expect(btn).toBeDisabled();
    await user.type(screen.getByTestId('disk-folder-input'), '/some/folder{Enter}');
    expect(screen.getByTestId('disk-folder-chip-/some/folder')).toBeInTheDocument();
    expect(btn).toBeEnabled();
    await user.click(screen.getByTestId('disk-folder-chip-/some/folder')); // removes it
    expect(btn).toBeDisabled();
  });

  it('scans the chosen drive and shows category bars with bytes and percentages', async () => {
    setup();
    await analyze();
    expect(api.current.paramsOf('disk_analyzer.scan')).toEqual([{ paths: ['/data'] }]);
    const order = screen
      .getAllByTestId(/^disk-cat-[a-z]+$/)
      .map((e) => e.getAttribute('data-testid'));
    expect(order).toEqual(['disk-cat-video', 'disk-cat-music', 'disk-cat-pictures', 'disk-cat-other']);
    expect(screen.getByTestId('disk-cat-video-bytes')).toHaveTextContent('4.9 KB');
    expect(screen.getByTestId('disk-cat-video-pct')).toHaveTextContent('79.3%');
    expect(screen.getByTestId('disk-cat-pictures-files')).toHaveTextContent('2 files');
    expect(screen.queryByTestId('disk-cat-email')).not.toBeInTheDocument();
    expect(screen.getByTestId('disk-summary')).toHaveTextContent('5 files');
  });

  it('opens a category, sorts, and deletes selected files after confirmation', async () => {
    setup();
    api.current.handlers['disk_analyzer.delete'] = (p) => {
      const paths = (p as { paths: string[] }).paths;
      return {
        results: paths.map((path) => ({ path, ok: true, bytes: 200 })),
        deleted: paths.length,
        freedBytes: 200,
        totals: { ...TOTALS, pictures: { files: 1, bytes: 100 } },
        totalFiles: 4,
        totalBytes: 6107,
      };
    };
    const user = await analyze();
    await user.click(screen.getByTestId('disk-cat-pictures'));
    await screen.findByTestId('disk-file-b.png');
    expect(api.current.paramsOf('disk_analyzer.files')[0]).toMatchObject({
      scanId: 'da-1',
      category: 'pictures',
      sort: 'size',
      offset: 0,
    });
    expect(screen.getByTestId('disk-files-count')).toHaveTextContent('2 files');
    expect(screen.getByTestId('disk-size-b.png')).toHaveTextContent('200 B');

    await user.selectOptions(screen.getByTestId('disk-sort'), 'name');
    await waitFor(() => expect(api.current.paramsOf('disk_analyzer.files').at(-1)).toMatchObject({ sort: 'name' }));

    await user.click(screen.getByTestId('disk-check-b.png'));
    await user.click(screen.getByTestId('btn-disk-delete'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('200 B will be permanently deleted');
    expect(api.current.paramsOf('disk_analyzer.delete')).toHaveLength(0);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('disk-note')).toHaveTextContent('Deleted 1 file, freed 200 B'));
    expect(api.current.paramsOf('disk_analyzer.delete')).toEqual([{ scanId: 'da-1', paths: ['/data/b.png'] }]);
    expect(screen.queryByTestId('disk-file-b.png')).not.toBeInTheDocument();
    expect(screen.getByTestId('disk-file-a.jpg')).toBeInTheDocument();

    // back on the category view the bars use the totals returned by the delete
    await user.click(screen.getByTestId('disk-files-back'));
    expect(screen.getByTestId('disk-cat-pictures-files')).toHaveTextContent('1 file');
    expect(screen.getByTestId('disk-summary')).toHaveTextContent('4 files');
  });

  it('reports files that could not be deleted and keeps them listed', async () => {
    setup();
    api.current.handlers['disk_analyzer.delete'] = () => ({
      results: [{ path: '/data/a.jpg', ok: false, error: 'the file changed since the analysis; run it again', bytes: 0 }],
      deleted: 0,
      freedBytes: 0,
      totals: TOTALS,
      totalFiles: 5,
      totalBytes: 6307,
    });
    const user = await analyze();
    await user.click(screen.getByTestId('disk-cat-pictures'));
    await user.click(await screen.findByTestId('disk-check-a.jpg'));
    await user.click(screen.getByTestId('btn-disk-delete'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('disk-failures')).toHaveTextContent('changed since the analysis'));
    expect(screen.getByTestId('disk-file-a.jpg')).toBeInTheDocument();
  });

  it('select-all ticks every listed file', async () => {
    setup();
    const user = await analyze();
    await user.click(screen.getByTestId('disk-cat-pictures'));
    await screen.findByTestId('disk-file-b.png');
    await user.click(screen.getByTestId('disk-select-all'));
    expect(screen.getByTestId('btn-disk-delete')).toHaveTextContent('Delete 2 files');
  });

  it('asks the server to open the folder of a file', async () => {
    setup();
    api.current.handlers['disk_analyzer.open_folder'] = () => ({ opened: '/data' });
    const user = await analyze();
    await user.click(screen.getByTestId('disk-cat-pictures'));
    await user.click(await screen.findByTestId('disk-open-a.jpg'));
    await waitFor(() => expect(api.current.paramsOf('disk_analyzer.open_folder')).toEqual([{ path: '/data/a.jpg' }]));
  });

  it('drills into folders with a breadcrumb', async () => {
    setup();
    const node = (path: string, name: string, bytes: number, children: TreeNode['children'], parent: string | null, canGoUp: boolean): TreeNode => ({
      path,
      name,
      bytes,
      files: 3,
      ownBytes: 10,
      ownFiles: 1,
      canGoUp,
      parent,
      childCount: children.length,
      children,
    });
    api.current.handlers['disk_analyzer.tree'] = (p) => {
      const path = (p as { path?: string }).path;
      if (path !== '/data/movies') {
        return node('/data', '/data', 6307, [{ path: '/data/movies', name: 'movies', bytes: 5000, files: 1 }, { path: '/data/pics', name: 'pics', bytes: 300, files: 2 }], null, false);
      }
      return node(path, 'movies', 5000, [], '/data', true);
    };
    const user = await analyze();
    await user.click(screen.getByTestId('disk-tab-folders'));
    await screen.findByTestId('disk-folder-movies');
    expect(screen.getByTestId('disk-crumbs')).toHaveTextContent('/data');
    expect(screen.getByTestId('disk-folder-bar-movies')).toHaveStyle({ width: '100%' });
    expect(screen.getByTestId('disk-folder-bar-pics')).toHaveStyle({ width: '6%' });
    await user.click(screen.getByTestId('disk-folder-movies'));
    await waitFor(() => expect(screen.getByTestId('disk-crumbs')).toHaveTextContent('movies'));
    expect(api.current.paramsOf('disk_analyzer.tree').at(-1)).toEqual({ scanId: 'da-1', path: '/data/movies' });
    await user.click(screen.getByTestId('disk-folder-up'));
    await waitFor(() => expect(screen.getByTestId('disk-folder-movies')).toBeInTheDocument());
    // list a folder's own files
    await user.click(screen.getByTestId('disk-folder-files'));
    await screen.findByTestId('disk-files');
    expect(api.current.paramsOf('disk_analyzer.files')[0]).toMatchObject({ folder: '/data' });
    await user.click(screen.getByTestId('disk-files-back'));
    expect(screen.getByTestId('disk-folders')).toBeInTheDocument();
  });

  it('can cancel a running analysis without an error', async () => {
    setup();
    api.current.handlers['disk_analyzer.scan'] = (_p, opts) => pendingUntilAborted(opts);
    const user = userEvent.setup();
    render(<DiskAnalyzerPage />);
    await user.click(await screen.findByTestId('disk-drive-/data'));
    await user.click(screen.getByTestId('btn-disk-analyze'));
    await user.click(await screen.findByTestId('btn-cancel'));
    await waitFor(() => expect(screen.queryByTestId('disk-progress')).not.toBeInTheDocument());
    expect(screen.queryByTestId('error-banner')).not.toBeInTheDocument();
    expect(screen.queryByTestId('disk-categories')).not.toBeInTheDocument();
  });

  it('shows scan errors in the banner', async () => {
    setup();
    api.current.handlers['disk_analyzer.scan'] = () => {
      throw Object.assign(new Error('not a folder: /x'), { code: 'InvalidParams' });
    };
    const user = userEvent.setup();
    render(<DiskAnalyzerPage />);
    await user.click(await screen.findByTestId('disk-drive-/data'));
    await user.click(screen.getByTestId('btn-disk-analyze'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('not a folder');
  });

  it('warns when only the largest files are listed', async () => {
    setup();
    api.current.handlers['disk_analyzer.scan'] = () => ({ ...SUMMARY, truncated: true, stored: 200_000 });
    await analyze();
    expect(screen.getByTestId('disk-truncated')).toHaveTextContent('200,000');
    const bars = within(screen.getByTestId('disk-categories'));
    expect(bars.getAllByRole('button')).toHaveLength(4);
  });
});
