import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DriverEntry, DriverUpdateReport } from '../api/driver_updater';
import { createApiMock, pendingUntilAborted } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import DriverUpdaterPage from './DriverUpdaterPage';

const FW: DriverEntry = {
  id: 'fwupd:3fc0',
  deviceName: 'System Firmware',
  currentVersion: '1.20.0',
  newVersion: '1.22.0',
  vendor: 'Dell',
  source: 'fwupd',
  description: 'Firmware for XPS',
  rebootRequired: true,
};
const GPU: DriverEntry = {
  id: 'ubuntu-drivers:nvidia-driver-535',
  deviceName: 'GA106 [GeForce RTX 3060]',
  newVersion: 'nvidia-driver-535',
  vendor: 'NVIDIA Corporation',
  source: 'ubuntu-drivers',
  description: 'Recommended proprietary driver nvidia-driver-535 is not installed',
};

let drivers: DriverEntry[];

function renderPage() {
  return render(
    <MemoryRouter>
      <DriverUpdaterPage />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  api.current = createApiMock();
  drivers = [FW, GPU];
  api.current.handlers['driver_updater.scan'] = () => drivers;
});

afterEach(() => vi.unstubAllGlobals());

describe('DriverUpdaterPage', () => {
  it('starts with a Scan prompt and does not scan by itself', async () => {
    renderPage();
    expect(screen.getByText('Scan for driver updates')).toBeInTheDocument();
    expect(screen.getByTestId('btn-scan')).toHaveTextContent('Scan');
    expect(api.current.calls).toHaveLength(0);
    expect(screen.getByTestId('btn-update-selected')).toBeDisabled();
  });

  it('scan lists devices with versions, vendor, description and restart hints', async () => {
    renderPage();
    const user = userEvent.setup();
    await user.click(screen.getByTestId('btn-scan'));
    const fw = await screen.findByTestId('driver-fwupd:3fc0');
    expect(within(fw).getByTestId('driver-name')).toHaveTextContent('System Firmware');
    expect(within(fw).getByTestId('driver-versions')).toHaveTextContent('Dell - 1.20.0 \u2192 1.22.0');
    expect(fw).toHaveTextContent('Firmware for XPS');
    expect(within(fw).getByTestId('driver-reboot')).toBeInTheDocument();
    expect(screen.getByTestId('btn-scan')).toHaveTextContent('Scan again');
    expect(screen.getByTestId('drivers-list')).toHaveTextContent('Available updates (2)');
  });

  it('shows an empty state when nothing needs updating', async () => {
    drivers = [];
    renderPage();
    await userEvent.setup().click(screen.getByTestId('btn-scan'));
    expect(await screen.findByTestId('drivers-empty')).toHaveTextContent('No driver updates found');
  });

  it('installs the selected drivers after confirmation and shows results and the reboot notice', async () => {
    const report: DriverUpdateReport = {
      results: [{ id: 'fwupd:3fc0', deviceName: 'System Firmware', ok: true, exitCode: 0, message: 'Successfully installed firmware', rebootRequired: true }],
      rebootRequired: true,
      succeeded: 1,
      failed: 0,
      backupPath: null,
    };
    api.current.handlers['driver_updater.update'] = () => {
      drivers = [GPU];
      return report;
    };
    renderPage();
    const user = userEvent.setup();
    await user.click(screen.getByTestId('btn-scan'));
    await user.click(await screen.findByTestId('driver-select-fwupd:3fc0'));
    expect(screen.getByTestId('btn-update-selected')).toHaveTextContent('Update selected (1)');
    await user.click(screen.getByTestId('btn-update-selected'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Install 1 driver update?');
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('failed firmware update');
    expect(api.current.paramsOf('driver_updater.update')).toHaveLength(0);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('driver_updater.update')).toEqual([{ ids: ['fwupd:3fc0'] }]));
    expect(await screen.findByTestId('reboot-notice')).toHaveTextContent('Restart your computer');
    expect(screen.getByTestId('drivers-report')).toHaveTextContent('1 succeeded');
    await waitFor(() => expect(screen.queryByTestId('driver-fwupd:3fc0')).toBeNull());
  });

  it('a failed install shows the failure on the row and no reboot notice', async () => {
    api.current.handlers['driver_updater.update'] = () => ({
      results: [{ id: 'fwupd:3fc0', deviceName: 'System Firmware', ok: false, exitCode: 1, message: 'fwupdmgr update failed (exit code 1): Battery level is too low', rebootRequired: false }],
      rebootRequired: false,
      succeeded: 0,
      failed: 1,
      backupPath: null,
    });
    renderPage();
    const user = userEvent.setup();
    await user.click(screen.getByTestId('btn-scan'));
    await user.click(await screen.findByTestId('driver-select-fwupd:3fc0'));
    await user.click(screen.getByTestId('btn-update-selected'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('driver-result-fwupd:3fc0')).toHaveTextContent('Battery level is too low');
    expect(screen.queryByTestId('reboot-notice')).toBeNull();
  });

  it('shows scan errors (for example fwupd not reachable)', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['driver_updater.scan'] = () => Promise.reject(new ApiCallError('Io', 'fwupdmgr get-updates failed (exit code 1)'));
    renderPage();
    await userEvent.setup().click(screen.getByTestId('btn-scan'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('fwupdmgr get-updates failed');
  });

  it('shows progress and cancel while scanning', async () => {
    api.current.handlers['driver_updater.scan'] = (_p, opts) => {
      opts.onProgress?.({ stage: 'scan', message: 'Checking Windows Update' });
      return pendingUntilAborted(opts);
    };
    renderPage();
    const user = userEvent.setup();
    await user.click(screen.getByTestId('btn-scan'));
    expect(await screen.findByTestId('drivers-progress-message')).toHaveTextContent('Checking Windows Update');
    await user.click(screen.getByTestId('btn-cancel'));
    await waitFor(() => expect(screen.queryByTestId('drivers-progress')).toBeNull());
  });

  it('hides "Back up drivers" outside Windows', () => {
    renderPage();
    expect(screen.queryByTestId('btn-backup')).toBeNull();
  });

  it('on Windows offers a confirmed driver backup', async () => {
    vi.stubGlobal('navigator', { userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' });
    api.current.handlers['driver_updater.backup'] = () => ({
      ok: true,
      path: 'C:\\data\\backups\\drivers-1',
      exitCode: 0,
      message: 'Drivers exported to C:\\data\\backups\\drivers-1',
    });
    renderPage();
    const user = userEvent.setup();
    await user.click(screen.getByTestId('btn-backup'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('Back up installed drivers?');
    expect(api.current.paramsOf('driver_updater.backup')).toHaveLength(0);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('backup-note')).toHaveTextContent('Drivers exported to C:\\data\\backups\\drivers-1');
  });
});
