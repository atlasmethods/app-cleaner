import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { RulesListing } from '../api/cleaner';
import type { Backend, Schedule } from '../api/scheduler';
import { createApiMock } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import SchedulesPage from './SchedulesPage';

const listing: RulesListing = {
  categories: [
    {
      category: 'browser',
      label: 'Browsers',
      groups: [
        {
          group: 'Google Chrome',
          rules: [
            { id: 'chrome.cache', name: 'Cache', description: '', enabled: true, defaultEnabled: true },
            { id: 'chrome.history', name: 'History', description: '', enabled: false, defaultEnabled: false },
          ],
        },
      ],
    },
    {
      category: 'system',
      label: 'System',
      groups: [{ group: 'Temporary files', rules: [{ id: 'temp.files', name: 'Temp', description: '', enabled: true, defaultEnabled: true }] }],
    },
  ],
};

const nightly: Schedule = {
  id: '0123456789abcdef',
  name: 'Nightly',
  enabled: true,
  frequency: 'weekly',
  time: '03:00',
  weekdays: [1, 3],
  dayOfMonth: 1,
  action: { kind: 'clean', rules: null },
  createdAt: '2024-01-01T00:00:00Z',
};

let store: Schedule[];
let backend: Backend;
let counter: number;

beforeEach(() => {
  api.current = createApiMock();
  store = [structuredClone(nightly)];
  backend = { kind: 'systemd', available: true, detail: 'Schedules run as systemd user timers.' };
  counter = 0;
  const h = api.current.handlers;
  h['scheduler.list'] = () => structuredClone(store);
  h['scheduler.backend'] = () => backend;
  h['cleaner.list_rules'] = () => listing;
  h['scheduler.add'] = (p) => {
    const input = p as Omit<Schedule, 'id' | 'createdAt'>;
    const s: Schedule = { ...input, id: `00000000000000${String(++counter).padStart(2, '0')}`, createdAt: '2024-02-01T00:00:00Z' };
    store.push(s);
    return structuredClone(s);
  };
  h['scheduler.update'] = (p) => {
    const { id, ...rest } = p as { id: string } & Partial<Schedule>;
    const i = store.findIndex((s) => s.id === id);
    store[i] = { ...store[i]!, ...rest };
    return structuredClone(store[i]);
  };
  h['scheduler.set_enabled'] = (p) => {
    const { id, enabled } = p as { id: string; enabled: boolean };
    const s = store.find((x) => x.id === id)!;
    s.enabled = enabled;
    return structuredClone(s);
  };
  h['scheduler.remove'] = (p) => {
    store = store.filter((s) => s.id !== (p as { id: string }).id);
    return { removed: true, warnings: [] };
  };
  h['scheduler.run_now'] = (p) => {
    const s = store.find((x) => x.id === (p as { id: string }).id)!;
    s.lastRun = '2024-03-01T12:00:00Z';
    s.lastResult = { ok: true, totalBytes: 3 * 1024 * 1024, totalFiles: 4 };
    return { schedule: structuredClone(s), report: { totalBytes: 3 * 1024 * 1024 } };
  };
});

function renderPage() {
  return render(
    <MemoryRouter>
      <SchedulesPage />
    </MemoryRouter>,
  );
}

const ID = nightly.id;

describe('SchedulesPage', () => {
  it('lists schedules in words with the last run and the backend note', async () => {
    store[0]!.lastRun = '2024-03-01T03:00:00Z';
    store[0]!.lastResult = { ok: true, totalBytes: 2048, totalFiles: 2 };
    renderPage();
    expect(await screen.findByTestId(`schedule-name-${ID}`)).toHaveTextContent('Nightly');
    expect(screen.getByTestId(`schedule-when-${ID}`)).toHaveTextContent('Every Mon and Wed at 03:00');
    expect(screen.getByTestId(`schedule-last-${ID}`)).toHaveTextContent(/Last run .*: cleaned 2\.0 KB/);
    expect(screen.getByTestId(`schedule-toggle-${ID}`)).toHaveAttribute('aria-checked', 'true');
    expect(await screen.findByTestId('schedules-backend')).toHaveTextContent('systemd user timers');
  });

  it('shows an empty state, and a warning when no OS scheduler works', async () => {
    store = [];
    backend = { kind: 'none', available: false, detail: 'Neither systemd user services nor cron are available.' };
    renderPage();
    expect(await screen.findByText('No schedules yet')).toBeInTheDocument();
    expect(await screen.findByTestId('schedules-backend')).toHaveTextContent('Neither systemd');
  });

  it('shows a failed last run and a never-run schedule', async () => {
    store.push({ ...nightly, id: 'fedcba9876543210', name: 'Broken', lastRun: '2024-03-01T03:00:00Z', lastResult: { ok: false, totalBytes: 0, totalFiles: 0, message: 'unknown rule `x`' } });
    renderPage();
    expect(await screen.findByTestId(`schedule-last-${ID}`)).toHaveTextContent('Has not run yet');
    expect(screen.getByTestId('schedule-last-fedcba9876543210')).toHaveTextContent('unknown rule `x`');
  });

  it('adds a weekly schedule', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('schedule-add'));
    const sheet = await screen.findByTestId('schedule-sheet');
    await user.type(within(sheet).getByTestId('schedule-name'), 'Weekend');
    await user.selectOptions(within(sheet).getByTestId('schedule-frequency'), 'weekly');
    // Monday is preselected; add Saturday and Sunday.
    await user.click(within(sheet).getByTestId('schedule-weekday-6'));
    await user.click(within(sheet).getByTestId('schedule-weekday-7'));
    await user.click(within(sheet).getByTestId('schedule-weekday-1'));
    fireTime(within(sheet).getByTestId('schedule-time'), '21:30');
    await user.click(within(sheet).getByTestId('schedule-save'));
    await waitFor(() => expect(screen.queryByTestId('schedule-sheet')).toBeNull());
    expect(api.current.paramsOf('scheduler.add')).toEqual([
      {
        name: 'Weekend',
        enabled: true,
        frequency: 'weekly',
        time: '21:30',
        weekdays: [6, 7],
        dayOfMonth: 1,
        action: { kind: 'clean', rules: null },
      },
    ]);
    const added = store[1]!;
    expect(await screen.findByTestId(`schedule-when-${added.id}`)).toHaveTextContent('Every Sat and Sun at 21:30');
  });

  it('monthly and hourly pickers', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('schedule-add'));
    await user.type(screen.getByTestId('schedule-name'), 'Monthly');
    await user.selectOptions(screen.getByTestId('schedule-frequency'), 'monthly');
    await user.selectOptions(screen.getByTestId('schedule-dom'), '15');
    expect(screen.queryByTestId('schedule-weekday-1')).toBeNull();
    await user.click(screen.getByTestId('schedule-save'));
    await waitFor(() => expect(api.current.paramsOf('scheduler.add')).toHaveLength(1));
    expect(api.current.paramsOf('scheduler.add')[0]).toMatchObject({ frequency: 'monthly', dayOfMonth: 15, time: '03:00' });

    await user.click(screen.getByTestId('schedule-add'));
    await user.type(screen.getByTestId('schedule-name'), 'Hourly');
    await user.selectOptions(screen.getByTestId('schedule-frequency'), 'hourly');
    expect(screen.queryByTestId('schedule-time')).toBeNull();
    const minute = screen.getByTestId('schedule-minute');
    await user.clear(minute);
    await user.type(minute, '20');
    await user.click(screen.getByTestId('schedule-save'));
    await waitFor(() => expect(api.current.paramsOf('scheduler.add')).toHaveLength(2));
    expect(api.current.paramsOf('scheduler.add')[1]).toMatchObject({ frequency: 'hourly', time: '00:20' });
  });

  it('on login needs no time', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('schedule-add'));
    await user.type(screen.getByTestId('schedule-name'), 'Login');
    await user.selectOptions(screen.getByTestId('schedule-frequency'), 'on_login');
    expect(screen.queryByTestId('schedule-time')).toBeNull();
    expect(screen.queryByTestId('schedule-minute')).toBeNull();
    await user.click(screen.getByTestId('schedule-save'));
    await waitFor(() => expect(api.current.paramsOf('scheduler.add')).toHaveLength(1));
    expect(await screen.findByText('Every time you log in')).toBeInTheDocument();
  });

  it('chooses specific things to clean', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('schedule-add'));
    await user.type(screen.getByTestId('schedule-name'), 'Browsers');
    await user.click(screen.getByTestId('schedule-rules-custom'));
    await user.click(await screen.findByTestId('schedule-rule-group-google-chrome'));
    await user.click(screen.getByTestId('schedule-save'));
    await waitFor(() => expect(api.current.paramsOf('scheduler.add')).toHaveLength(1));
    expect(api.current.paramsOf('scheduler.add')[0]).toMatchObject({
      action: { kind: 'clean', rules: expect.arrayContaining(['chrome.cache', 'chrome.history']) },
    });
  });

  it('blocks invalid drafts with a message and does not call the server', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('schedule-add'));
    await user.click(screen.getByTestId('schedule-save'));
    expect(await screen.findByTestId('schedule-problem')).toHaveTextContent('name');
    await user.type(screen.getByTestId('schedule-name'), 'nightly');
    await user.click(screen.getByTestId('schedule-save'));
    expect(screen.getByTestId('schedule-problem')).toHaveTextContent('already a schedule named');
    await user.clear(screen.getByTestId('schedule-name'));
    await user.type(screen.getByTestId('schedule-name'), 'Custom');
    await user.click(screen.getByTestId('schedule-rules-custom'));
    await user.click(screen.getByTestId('schedule-save'));
    expect(screen.getByTestId('schedule-problem')).toHaveTextContent('at least one thing');
    expect(api.current.paramsOf('scheduler.add')).toHaveLength(0);
  });

  it('shows a server error in the sheet and keeps it open', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['scheduler.add'] = () => Promise.reject(new ApiCallError('Io', 'systemctl enable failed: boom'));
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId('schedule-add'));
    await user.type(screen.getByTestId('schedule-name'), 'X');
    await user.click(screen.getByTestId('schedule-save'));
    const sheet = await screen.findByTestId('schedule-sheet');
    expect(await within(sheet).findByTestId('error-banner')).toHaveTextContent('boom');
    expect(screen.getByTestId('schedule-name')).toHaveValue('X');
  });

  it('edits a schedule', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`schedule-edit-${ID}`));
    const sheet = await screen.findByTestId('schedule-sheet');
    expect(within(sheet).getByTestId('schedule-sheet-title')).toHaveTextContent('Edit schedule');
    expect(within(sheet).getByTestId('schedule-name')).toHaveValue('Nightly');
    expect(within(sheet).getByTestId('schedule-weekday-1')).toHaveAttribute('aria-pressed', 'true');
    expect(within(sheet).getByTestId('schedule-weekday-2')).toHaveAttribute('aria-pressed', 'false');
    await user.click(within(sheet).getByTestId('schedule-weekday-5'));
    fireTime(within(sheet).getByTestId('schedule-time'), '04:45');
    await user.click(within(sheet).getByTestId('schedule-save'));
    await waitFor(() => expect(screen.queryByTestId('schedule-sheet')).toBeNull());
    expect(api.current.paramsOf('scheduler.update')[0]).toMatchObject({ id: ID, name: 'Nightly', weekdays: [1, 3, 5], time: '04:45', enabled: true });
    expect(screen.getByTestId(`schedule-when-${ID}`)).toHaveTextContent('Every Mon, Wed and Fri at 04:45');
  });

  it('enables and disables', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`schedule-toggle-${ID}`));
    await waitFor(() => expect(screen.getByTestId(`schedule-toggle-${ID}`)).toHaveAttribute('aria-checked', 'false'));
    expect(api.current.paramsOf('scheduler.set_enabled')).toEqual([{ id: ID, enabled: false }]);
  });

  it('runs now and reports what was cleaned', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`schedule-run-${ID}`));
    expect(await screen.findByTestId('schedules-note')).toHaveTextContent('Nightly: cleaned 3.0 MB');
    expect(screen.getByTestId(`schedule-last-${ID}`)).toHaveTextContent(/Last run .*cleaned 3\.0 MB/);
  });

  it('a failing run shows the error and refreshes the recorded result', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['scheduler.run_now'] = () => {
      store[0]!.lastRun = '2024-03-01T12:00:00Z';
      store[0]!.lastResult = { ok: false, totalBytes: 0, totalFiles: 0, message: 'unknown rule `gone`' };
      return Promise.reject(new ApiCallError('InvalidParams', 'unknown rule `gone` on this system'));
    };
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`schedule-run-${ID}`));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('unknown rule `gone`');
    await waitFor(() => expect(screen.getByTestId(`schedule-last-${ID}`)).toHaveTextContent('unknown rule `gone`'));
  });

  it('deletes only after confirmation', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`schedule-delete-${ID}`));
    expect(screen.getByRole('dialog')).toHaveTextContent('Delete "Nightly"?');
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(api.current.paramsOf('scheduler.remove')).toHaveLength(0);
    expect(screen.getByTestId(`schedule-${ID}`)).toBeInTheDocument();
    await user.click(screen.getByTestId(`schedule-delete-${ID}`));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.queryByTestId(`schedule-${ID}`)).toBeNull());
    expect(api.current.paramsOf('scheduler.remove')).toEqual([{ id: ID }]);
    expect(await screen.findByText('No schedules yet')).toBeInTheDocument();
  });

  it('reports leftovers of a removal', async () => {
    api.current.handlers['scheduler.remove'] = () => ({ removed: true, warnings: ['crontab busy'] });
    renderPage();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId(`schedule-delete-${ID}`));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('schedules-note')).toHaveTextContent('crontab busy');
  });

  it('shows a load error with retry', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['scheduler.list'] = () => Promise.reject(new ApiCallError('Io', 'schedules.json is damaged'));
    renderPage();
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('damaged');
  });
});

/** `<input type="time">` only accepts HH:MM; set it like the browser would. */
function fireTime(el: HTMLElement, value: string) {
  const input = el as HTMLInputElement;
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
  setter.call(input, value);
  input.dispatchEvent(new Event('input', { bubbles: true }));
}
