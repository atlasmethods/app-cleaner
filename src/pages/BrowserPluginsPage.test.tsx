import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Plugin } from '../api/browser_plugins';
import { ApiCallError } from '../lib/transport';
import { createApiMock } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import BrowserPluginsPage from './BrowserPluginsPage';

const plugin = (over: Partial<Plugin> & { id: string }): Plugin => ({
  browser: 'chrome',
  browserLabel: 'Google Chrome',
  profile: 'Default',
  extensionId: over.id,
  name: over.id,
  version: '1.0',
  description: '',
  enabled: true,
  type: 'extension',
  installLocation: '/x',
  canDisable: true,
  canRemove: true,
  running: false,
  ...over,
});

const ABP = plugin({ id: 'chrome:Default:abp', name: 'Ad Blocker', description: 'Blocks ads', version: '1.2.3' });
const SEC = plugin({
  id: 'chrome:Default:sec',
  name: 'Protected Ext',
  canDisable: false,
  note: "This browser protects extension settings; disable it from the browser's extensions page",
});
const FOX = plugin({ id: 'firefox:x.default:ubo', browser: 'firefox', browserLabel: 'Firefox', profile: 'x.default', name: 'uBlock Origin', type: 'theme' });

let plugins: Plugin[];

function renderPage() {
  return render(
    <MemoryRouter>
      <BrowserPluginsPage />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  api.current = createApiMock();
  plugins = [ABP, SEC, FOX];
  api.current.handlers['browser_plugins.list'] = () => plugins;
});

describe('BrowserPluginsPage', () => {
  it('groups add-ons by browser and profile with version, type and description', async () => {
    renderPage();
    await screen.findByTestId('plugins-browser-chrome');
    expect(screen.getByTestId('plugins-browser-firefox')).toHaveTextContent('Firefox');
    const row = screen.getByTestId(`plugin-row-${ABP.id}`);
    expect(row).toHaveTextContent('Ad Blocker');
    expect(row).toHaveTextContent('Version 1.2.3 - Blocks ads');
    expect(within(row).getByTestId('plugin-type')).toHaveTextContent('Extension');
    expect(within(screen.getByTestId(`plugin-row-${FOX.id}`)).getByTestId('plugin-type')).toHaveTextContent('Theme');
  });

  it('disables the toggle with an explanation where the browser protects the setting', async () => {
    renderPage();
    await screen.findByTestId('plugins-browser-chrome');
    expect(screen.getByTestId(`plugin-toggle-${SEC.id}`)).toBeDisabled();
    expect(within(screen.getByTestId(`plugin-row-${SEC.id}`)).getByTestId('plugin-note')).toHaveTextContent('protects extension settings');
    expect(screen.getByTestId(`plugin-toggle-${ABP.id}`)).toBeEnabled();
  });

  it('toggles an add-on and uses the server state', async () => {
    api.current.handlers['browser_plugins.set_enabled'] = (p) => ({
      ok: true,
      plugin: { ...ABP, enabled: (p as { enabled: boolean }).enabled },
    });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('plugins-browser-chrome');
    await user.click(screen.getByTestId(`plugin-toggle-${ABP.id}`));
    await waitFor(() => expect(screen.getByTestId(`plugin-toggle-${ABP.id}`)).toHaveAttribute('aria-checked', 'false'));
    expect(api.current.paramsOf('browser_plugins.set_enabled')).toEqual([{ id: ABP.id, enabled: false }]);
  });

  it('shows a friendly "close the browser" notice instead of a raw error', async () => {
    api.current.handlers['browser_plugins.set_enabled'] = () => {
      throw new ApiCallError('PermissionDenied', 'Close Google Chrome first: while it is running it would undo this change.');
    };
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('plugins-browser-chrome');
    await user.click(screen.getByTestId(`plugin-toggle-${ABP.id}`));
    expect(await screen.findByTestId('plugins-close-notice')).toHaveTextContent('Close Google Chrome first');
    expect(screen.queryByTestId('error-banner')).toBeNull();
    expect(screen.getByTestId(`plugin-toggle-${ABP.id}`)).toHaveAttribute('aria-checked', 'true');
  });

  it('marks a running browser', async () => {
    plugins = [{ ...ABP, running: true }];
    renderPage();
    expect(await screen.findByTestId('plugins-running-chrome')).toHaveTextContent('close it to make changes');
  });

  it('removes after a confirmation and mentions the backup', async () => {
    api.current.handlers['browser_plugins.remove'] = () => ({ ok: true, backupId: 'plugins-7', backupPath: '/x', note: null });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('plugins-browser-chrome');
    expect(screen.queryByTestId(`plugin-remove-${FOX.id}`)).toBeInTheDocument();
    await user.click(screen.getByTestId(`plugin-remove-${ABP.id}`));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Remove Ad Blocker?');
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('backup');
    expect(api.current.paramsOf('browser_plugins.remove')).toEqual([]);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.queryByTestId(`plugin-row-${ABP.id}`)).toBeNull());
    expect(screen.getByTestId('plugins-note')).toHaveTextContent('plugins-7');
  });

  it('offers Undo after a removal and restores the add-on from its backup', async () => {
    api.current.handlers['browser_plugins.remove'] = () => {
      plugins = plugins.filter((p) => p.id !== ABP.id);
      return { ok: true, backupId: 'plugins-8', backupPath: '/x', note: null };
    };
    api.current.handlers['browser_plugins.restore_backup'] = () => {
      plugins = [ABP, SEC, FOX];
      return { ok: true, id: 'plugins-8', name: 'Ad Blocker', browser: 'Google Chrome', restored: 1, notes: [] };
    };
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('plugins-browser-chrome');
    expect(screen.queryByTestId('plugins-undo')).toBeNull();
    await user.click(screen.getByTestId(`plugin-remove-${ABP.id}`));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.queryByTestId(`plugin-row-${ABP.id}`)).toBeNull());
    await user.click(screen.getByTestId('plugins-undo'));
    await waitFor(() => expect(screen.getByTestId(`plugin-row-${ABP.id}`)).toBeInTheDocument());
    expect(api.current.paramsOf('browser_plugins.restore_backup')).toEqual([{ id: 'plugins-8' }]);
    expect(screen.getByTestId('plugins-note')).toHaveTextContent('Ad Blocker is back.');
    expect(screen.queryByTestId('plugins-undo')).toBeNull();
  });

  it('asks to close the browser when Undo is refused and keeps Undo available', async () => {
    api.current.handlers['browser_plugins.remove'] = () => ({ ok: true, backupId: 'plugins-9', backupPath: '/x', note: null });
    api.current.handlers['browser_plugins.restore_backup'] = () => {
      throw new ApiCallError('PermissionDenied', 'Close Google Chrome first: while it is running it would undo this change.');
    };
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('plugins-browser-chrome');
    await user.click(screen.getByTestId(`plugin-remove-${ABP.id}`));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await user.click(await screen.findByTestId('plugins-undo'));
    expect(await screen.findByTestId('plugins-close-notice')).toHaveTextContent('Close Google Chrome first');
    expect(screen.getByTestId('plugins-undo')).toBeInTheDocument();
    expect(screen.queryByTestId('error-banner')).toBeNull();
  });

  it('searches and shows an empty state', async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('plugins-browser-chrome');
    await user.type(screen.getByTestId('plugins-search'), 'ublock');
    expect(screen.queryByTestId('plugins-browser-chrome')).toBeNull();
    expect(screen.getByTestId('plugins-browser-firefox')).toBeInTheDocument();
    await user.clear(screen.getByTestId('plugins-search'));
    await user.type(screen.getByTestId('plugins-search'), 'zzzz');
    expect(screen.getByTestId('plugins-empty')).toHaveTextContent('Nothing matches');
  });

  it('shows other errors in the banner', async () => {
    api.current.handlers['browser_plugins.list'] = () => {
      throw new ApiCallError('Io', 'profile unreadable');
    };
    renderPage();
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('profile unreadable');
  });
});
