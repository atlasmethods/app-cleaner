import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SecureDeleteResult } from '../api/secure_delete';
import { createApiMock } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import FileShredderPage from './FileShredderPage';

function renderPage() {
  return render(
    <MemoryRouter>
      <FileShredderPage />
    </MemoryRouter>,
  );
}

async function addPath(user: ReturnType<typeof userEvent.setup>, text: string) {
  await user.click(screen.getByTestId('shredder-input'));
  await user.paste(text);
  await user.click(screen.getByTestId('shredder-add'));
}

describe('FileShredderPage', () => {
  beforeEach(() => {
    api.current = createApiMock();
    api.current.handlers['settings.get'] = () => ({ secureDelete: { enabled: false, passes: 3 } });
  });

  it('starts empty with the shred button disabled, and uses the saved passes as the default', async () => {
    renderPage();
    expect(screen.getByTestId('btn-shredder-start')).toBeDisabled();
    await waitFor(() => expect(screen.getByTestId('shredder-passes')).toHaveValue('3'));
  });

  it('rejects relative paths and adds several pasted absolute ones once each', async () => {
    const user = userEvent.setup();
    renderPage();
    await addPath(user, 'notes.txt');
    expect(screen.getByTestId('shredder-input-error')).toHaveTextContent('absolute');
    expect(screen.queryAllByTestId('shredder-item')).toHaveLength(0);
    await user.clear(screen.getByTestId('shredder-input'));
    await addPath(user, '/a/one.txt\n/a/two.txt\n/a/one.txt');
    expect(screen.getAllByTestId('shredder-item')).toHaveLength(2);
    expect(screen.getByTestId('btn-shredder-start')).toHaveTextContent('Shred 2 items');
    await user.click(screen.getByRole('button', { name: 'Remove /a/two.txt' }));
    expect(screen.getAllByTestId('shredder-item')).toHaveLength(1);
  });

  it('needs the typed word, sends paths + passes, shows per-path results and keeps failures', async () => {
    const user = userEvent.setup();
    const result: SecureDeleteResult = {
      totalBytes: 2048,
      results: [
        { path: '/a/one.txt', ok: true, bytes: 2048 },
        { path: '/a/two.txt', ok: false, error: 'refused: path is protected', bytes: 0 },
      ],
    };
    api.current.handlers['secure_delete.delete'] = () => result;
    renderPage();
    await addPath(user, '/a/one.txt\n/a/two.txt');
    await user.selectOptions(screen.getByTestId('shredder-passes'), '7');
    await user.click(screen.getByTestId('btn-shredder-start'));
    expect(screen.getByTestId('confirm-sheet-confirm')).toBeDisabled();
    expect(api.current.paramsOf('secure_delete.delete')).toHaveLength(0);
    await user.type(screen.getByTestId('confirm-sheet-input'), 'SHRED');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('shredder-result')).toBeInTheDocument());
    expect(api.current.paramsOf('secure_delete.delete')).toEqual([{ paths: ['/a/one.txt', '/a/two.txt'], passes: 7 }]);
    expect(screen.getByTestId('shredder-summary')).toHaveTextContent('Shredded 1 item (2.0 KB). 1 could not be shredded.');
    expect(screen.getByTestId('shredder-result-failed')).toHaveTextContent('refused: path is protected');
    // only the failed path stays selected
    expect(screen.getAllByTestId('shredder-item')).toHaveLength(1);
    expect(screen.getByTestId('shredder-item')).toHaveTextContent('/a/two.txt');
  });

  it('shows the backend error', async () => {
    const user = userEvent.setup();
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['secure_delete.delete'] = () => Promise.reject(new ApiCallError('InvalidParams', '`paths` is empty'));
    renderPage();
    await addPath(user, '/a/x');
    await user.click(screen.getByTestId('btn-shredder-start'));
    await user.type(screen.getByTestId('confirm-sheet-input'), 'SHRED');
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('`paths` is empty');
  });
});
