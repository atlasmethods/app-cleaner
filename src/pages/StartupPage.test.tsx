import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { StartupItem } from '../api/startup';
import { createApiMock } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import StartupPage from './StartupPage';

const item = (over: Partial<StartupItem> & { id: string }): StartupItem => ({
  name: over.id,
  command: '/usr/bin/' + over.id,
  location: '/home/u/.config/autostart/' + over.id + '.desktop',
  kind: 'autostart',
  scope: 'user',
  enabled: true,
  impact: 'unknown',
  canDisable: true,
  canDelete: true,
  critical: false,
  ...over,
});

const SLACK = item({ id: 'xdg:user:slack.desktop', name: 'Slack', impact: 'high', publisher: 'Slack Technologies' });
const SSH = item({
  id: 'systemd:system:ssh.service',
  name: 'ssh',
  kind: 'service',
  scope: 'system',
  canDelete: false,
  warning: 'Disabling SSH stops remote logins.',
});
const DBUS = item({ id: 'systemd:system:dbus.service', name: 'dbus', kind: 'service', scope: 'system', critical: true, canDisable: false, canDelete: false });
const CRON = item({ id: 'cron:1:0', name: 'sync.sh', kind: 'cron', command: '/home/u/sync.sh' });

let items: StartupItem[];

function renderPage() {
  return render(
    <MemoryRouter>
      <StartupPage />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  api.current = createApiMock();
  items = [SLACK, SSH, DBUS, CRON];
  api.current.handlers['startup.list'] = () => items;
});

describe('StartupPage', () => {
  it('lists items with impact badges, chips only for present kinds, and hides system items', async () => {
    renderPage();
    await screen.findByTestId('startup-list');
    expect(screen.queryAllByTestId('startup-name').map((e) => e.textContent)).toEqual(['Slack', 'ssh', 'sync.sh']);
    expect(within(screen.getByTestId(`startup-row-${SLACK.id}`)).getByTestId('startup-impact')).toHaveTextContent('High');
    expect(screen.getByTestId('chip-all')).toHaveTextContent('All (3)');
    expect(screen.getByTestId('chip-autostart')).toBeInTheDocument();
    expect(screen.getByTestId('chip-service')).toHaveTextContent('Services (1)');
    expect(screen.getByTestId('chip-cron')).toBeInTheDocument();
    expect(screen.queryByTestId('chip-context_menu')).toBeNull();
    expect(screen.getByText(/Show system items \(1\)/)).toBeInTheDocument();
  });

  it('shows critical items locked and disabled once system items are shown', async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('startup-list');
    await user.click(screen.getByTestId('startup-show-system'));
    const row = screen.getByTestId(`startup-row-${DBUS.id}`);
    expect(row).toBeInTheDocument();
    expect(within(row).getByLabelText('System item')).toBeInTheDocument();
    expect(screen.getByTestId(`startup-toggle-${DBUS.id}`)).toBeDisabled();
  });

  it('filters by chip and by search', async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('startup-list');
    await user.click(screen.getByTestId('chip-cron'));
    expect(screen.queryAllByTestId('startup-name').map((e) => e.textContent)).toEqual(['sync.sh']);
    await user.click(screen.getByTestId('chip-all'));
    await user.type(screen.getByTestId('startup-search'), 'slack tech');
    expect(screen.queryAllByTestId('startup-name').map((e) => e.textContent)).toEqual(['Slack']);
    await user.clear(screen.getByTestId('startup-search'));
    await user.type(screen.getByTestId('startup-search'), 'nomatch');
    expect(screen.getByTestId('startup-empty')).toHaveTextContent('Nothing matches');
  });

  it('turns a personal item off straight away and shows the server state', async () => {
    api.current.handlers['startup.set_enabled'] = (p) => ({
      ok: true,
      item: { ...SLACK, enabled: (p as { enabled: boolean }).enabled },
    });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('startup-list');
    const sw = screen.getByTestId(`startup-toggle-${SLACK.id}`);
    expect(sw).toHaveAttribute('aria-checked', 'true');
    await user.click(sw);
    await waitFor(() => expect(screen.getByTestId(`startup-toggle-${SLACK.id}`)).toHaveAttribute('aria-checked', 'false'));
    expect(api.current.paramsOf('startup.set_enabled')).toEqual([{ id: SLACK.id, enabled: false }]);
    expect(screen.getByTestId('startup-note')).toHaveTextContent('Slack is now off');
  });

  it('asks before changing a machine-wide item that has a warning', async () => {
    api.current.handlers['startup.set_enabled'] = () => ({ ok: true, item: { ...SSH, enabled: false } });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('startup-list');
    await user.click(screen.getByTestId(`startup-toggle-${SSH.id}`));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Disabling SSH stops remote logins.');
    expect(api.current.paramsOf('startup.set_enabled')).toEqual([]);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('startup.set_enabled')).toHaveLength(1));
  });

  it('deletes through a confirmation that mentions the backup', async () => {
    api.current.handlers['startup.remove'] = () => ({ ok: true, id: SLACK.id, backupId: 'startup-1', backupPath: '/x' });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('startup-list');
    await user.click(screen.getByTestId(`startup-menu-${SLACK.id}`));
    expect(screen.getByTestId('startup-sheet-command')).toHaveTextContent('/usr/bin/xdg:user:slack.desktop');
    await user.click(screen.getByTestId('startup-action-delete'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('A backup is saved first');
    expect(api.current.paramsOf('startup.remove')).toEqual([]);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.queryByTestId(`startup-row-${SLACK.id}`)).toBeNull());
    expect(screen.getByTestId('startup-note')).toHaveTextContent('Backup saved as startup-1');
  });

  it('offers Undo after a delete and puts the item back from its backup', async () => {
    api.current.handlers['startup.remove'] = () => {
      items = items.filter((i) => i.id !== SLACK.id);
      return { ok: true, id: SLACK.id, backupId: 'startup-17', backupPath: '/x' };
    };
    api.current.handlers['startup.restore_backup'] = () => {
      items = [SLACK, SSH, DBUS, CRON];
      return { ok: true, id: 'startup-17', restored: 1, notes: [], name: 'Slack' };
    };
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('startup-list');
    expect(screen.queryByTestId('startup-undo')).toBeNull();
    await user.click(screen.getByTestId(`startup-menu-${SLACK.id}`));
    await user.click(screen.getByTestId('startup-action-delete'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.queryByTestId(`startup-row-${SLACK.id}`)).toBeNull());
    await user.click(screen.getByTestId('startup-undo'));
    await waitFor(() => expect(screen.getByTestId(`startup-row-${SLACK.id}`)).toBeInTheDocument());
    expect(api.current.paramsOf('startup.restore_backup')).toEqual([{ id: 'startup-17' }]);
    expect(screen.getByTestId('startup-note')).toHaveTextContent('Slack is back.');
    expect(screen.queryByTestId('startup-undo')).toBeNull();
  });

  it('says so when Undo could not put the item back, and keeps Undo after an error', async () => {
    api.current.handlers['startup.remove'] = () => ({ ok: true, id: SLACK.id, backupId: 'startup-18', backupPath: '/x' });
    let n = 0;
    api.current.handlers['startup.restore_backup'] = () => {
      n += 1;
      if (n === 1) throw Object.assign(new Error('x'), { code: 'Io', message: 'could not read the backup' });
      return { ok: true, id: 'startup-18', restored: 0, notes: ['/home/u/.config/autostart/slack.desktop already exists; left as it is'], name: 'Slack' };
    };
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('startup-list');
    await user.click(screen.getByTestId(`startup-menu-${SLACK.id}`));
    await user.click(screen.getByTestId('startup-action-delete'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await user.click(await screen.findByTestId('startup-undo'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('could not read the backup');
    await user.click(screen.getByTestId('startup-undo'));
    await waitFor(() => expect(screen.getByTestId('startup-note')).toHaveTextContent('was not put back'));
    expect(screen.getByTestId('startup-note')).toHaveTextContent('already exists');
  });

  it('does not offer delete for items that can only be switched', async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('startup-list');
    await user.click(screen.getByTestId(`startup-menu-${SSH.id}`));
    expect(screen.queryByTestId('startup-action-delete')).toBeNull();
  });

  it('shows failures in the error banner and keeps the list', async () => {
    api.current.handlers['startup.set_enabled'] = () => {
      throw Object.assign(new Error('boom'), { code: 'Io', message: 'systemctl disable failed' });
    };
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('startup-list');
    await user.click(screen.getByTestId(`startup-toggle-${SLACK.id}`));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('systemctl disable failed');
    expect(screen.getByTestId(`startup-toggle-${SLACK.id}`)).toHaveAttribute('aria-checked', 'true');
  });

  it('shows an empty state and a load error with retry', async () => {
    items = [];
    renderPage();
    expect(await screen.findByTestId('startup-empty')).toHaveTextContent('Nothing starts automatically');
  });
});
