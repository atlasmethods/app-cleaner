import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DeviceListing, WiperDrive } from '../api/wiper';
import { createApiMock, pendingUntilAborted } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import DriveWiperPage from './DriveWiperPage';

const DRIVES: WiperDrive[] = [
  { name: 'sda2', mount: '/', fs: 'ext4', total: 100e9, available: 40e9, removable: false, isSystem: true, device: '/dev/sda2', wholeDisk: '/dev/sda' },
  { name: 'sdb1', mount: '/media/usb', fs: 'vfat', total: 8e9, available: 2e9, removable: true, isSystem: false, device: '/dev/sdb1', wholeDisk: '/dev/sdb' },
];

const dev = (device: string, over: Partial<DeviceListing['devices'][number]> = {}): DeviceListing['devices'][number] => ({
  device,
  name: device.replace('/dev/', ''),
  sizeBytes: 8e9,
  removable: true,
  model: 'Disk',
  partitions: [],
  mounts: [],
  swap: false,
  holders: [],
  isSystem: false,
  ...over,
});

const DEVICES: DeviceListing = {
  supported: true,
  devices: [
    dev('/dev/sda', { isSystem: true, mounts: ['/', '/boot/efi'], removable: false }),
    dev('/dev/sdb', { mounts: ['/media/usb'] }),
    dev('/dev/sdc'),
  ],
};

beforeEach(() => {
  api.current = createApiMock();
  api.current.handlers['wiper.list_drives'] = () => DRIVES;
  api.current.handlers['wiper.list_devices'] = () => DEVICES;
});

async function open() {
  const user = userEvent.setup();
  render(<DriveWiperPage />);
  await screen.findByTestId('wiper-drive-/');
  return user;
}

describe('DriveWiperPage - free space', () => {
  it('lists drives with a System badge and disables Wipe until one is chosen', async () => {
    await open();
    expect(screen.getAllByTestId('wiper-system-badge')).toHaveLength(1);
    expect(screen.getByTestId('wiper-drive-/')).toHaveTextContent('System');
    expect(screen.getByTestId('wiper-drive-/media/usb')).toHaveTextContent('Removable');
    expect(screen.getByTestId('btn-wiper-start')).toBeDisabled();
    expect(screen.getByTestId('wiper-warning')).toHaveTextContent('fills the drive');
  });

  it('offers 1, 3, 7 and 35 passes', async () => {
    await open();
    const options = Array.from(screen.getByTestId('wiper-passes').querySelectorAll('option'));
    expect(options.map((o) => o.value)).toEqual(['1', '3', '7', '35']);
    expect(options[1]?.textContent).toContain('DoD 5220.22-M');
    expect(options[3]?.textContent).toContain('Gutmann');
  });

  it('wipes free space after confirmation and reports the result', async () => {
    api.current.handlers['wiper.wipe_free_space'] = () => ({
      mount: '/media/usb',
      passes: 3,
      filesWritten: 2,
      bytesPerPass: 2e9,
      bytesWritten: 6e9,
      freeBefore: 2e9,
      freeAfter: 2e9,
      location: '/media/usb',
      staleRemoved: 0,
      durationMs: 65_000,
    });
    const user = await open();
    await user.click(screen.getByTestId('wiper-drive-/media/usb'));
    await user.selectOptions(screen.getByTestId('wiper-passes'), '3');
    await user.click(screen.getByTestId('btn-wiper-start'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('/media/usb');
    expect(api.current.paramsOf('wiper.wipe_free_space')).toHaveLength(0);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('wiper-result')).toHaveTextContent('Wiped 1.9 GB of free space'));
    expect(api.current.paramsOf('wiper.wipe_free_space')).toEqual([{ mount: '/media/usb', passes: 3 }]);
    expect(screen.getByTestId('wiper-result')).toHaveTextContent('3 passes');
  });

  it('warns about wiping the system drive but allows it', async () => {
    const user = await open();
    await user.click(screen.getByTestId('wiper-drive-/'));
    expect(screen.getByTestId('wiper-system-note')).toBeInTheDocument();
    expect(screen.getByTestId('btn-wiper-start')).toBeEnabled();
  });

  it('cancelling is offered while running and ends without an error', async () => {
    api.current.handlers['wiper.wipe_free_space'] = (_p, opts) => pendingUntilAborted(opts);
    const user = await open();
    await user.click(screen.getByTestId('wiper-drive-/media/usb'));
    await user.click(screen.getByTestId('btn-wiper-start'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('wiper-progress')).toHaveTextContent('gives the space back');
    await user.click(screen.getByTestId('btn-cancel'));
    await waitFor(() => expect(screen.queryByTestId('wiper-progress')).not.toBeInTheDocument());
    expect(screen.queryByTestId('error-banner')).not.toBeInTheDocument();
  });

  it('shows the server error', async () => {
    api.current.handlers['wiper.wipe_free_space'] = () => {
      throw Object.assign(new Error('cannot write to /media/usb'), { code: 'PermissionDenied' });
    };
    const user = await open();
    await user.click(screen.getByTestId('wiper-drive-/media/usb'));
    await user.click(screen.getByTestId('btn-wiper-start'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('cannot write');
  });
});

describe('DriveWiperPage - entire drive', () => {
  async function drive() {
    const user = await open();
    await user.click(screen.getByTestId('wiper-mode-drive'));
    await screen.findByTestId('wiper-device-/dev/sdc');
    return user;
  }

  it('shows a strong warning and disables system drives and drives in use', async () => {
    await drive();
    expect(screen.getByTestId('wiper-warning')).toHaveTextContent('destroys EVERYTHING');
    expect(screen.getByTestId('wiper-device-/dev/sda')).toBeDisabled();
    expect(screen.getByTestId('wiper-reason-/dev/sda')).toHaveTextContent('System drive');
    expect(screen.getByTestId('wiper-device-/dev/sdb')).toBeDisabled();
    expect(screen.getByTestId('wiper-reason-/dev/sdb')).toHaveTextContent('mounted at /media/usb');
    expect(screen.getByTestId('wiper-device-/dev/sdc')).toBeEnabled();
  });

  it('needs the exact device path typed before the button enables', async () => {
    const user = await drive();
    await user.click(screen.getByTestId('wiper-device-/dev/sdc'));
    const start = screen.getByTestId('btn-wiper-start');
    expect(start).toBeDisabled();
    const input = screen.getByTestId('wiper-confirm-input');
    for (const wrong of ['sdc', '/dev/sd', '/dev/sdc ', '/dev/SDC']) {
      await user.clear(input);
      await user.type(input, wrong);
      expect(start, wrong).toBeDisabled();
    }
    await user.clear(input);
    await user.type(input, '/dev/sdc');
    expect(start).toBeEnabled();
  });

  it('erases after the typed confirmation and a second confirmation', async () => {
    api.current.handlers['wiper.wipe_drive'] = () => ({ device: '/dev/sdc', passes: 1, bytes: 8e9, durationMs: 1000 });
    const user = await drive();
    await user.click(screen.getByTestId('wiper-device-/dev/sdc'));
    await user.type(screen.getByTestId('wiper-confirm-input'), '/dev/sdc');
    await user.click(screen.getByTestId('btn-wiper-start'));
    expect(screen.getByTestId('confirm-sheet')).toHaveTextContent('destroyed permanently');
    expect(api.current.paramsOf('wiper.wipe_drive')).toHaveLength(0);
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('wiper-result')).toHaveTextContent('/dev/sdc was overwritten'));
    expect(api.current.paramsOf('wiper.wipe_drive')).toEqual([{ device: '/dev/sdc', passes: 1, confirm: '/dev/sdc' }]);
  });

  it('changing the selected drive clears the typed confirmation', async () => {
    api.current.handlers['wiper.list_devices'] = () => ({
      supported: true,
      devices: [dev('/dev/sdc'), dev('/dev/sdd')],
    });
    const user = await open();
    await user.click(screen.getByTestId('wiper-mode-drive'));
    await user.click(await screen.findByTestId('wiper-device-/dev/sdc'));
    await user.type(screen.getByTestId('wiper-confirm-input'), '/dev/sdc');
    await user.click(screen.getByTestId('wiper-device-/dev/sdd'));
    expect(screen.getByTestId('wiper-confirm-input')).toHaveValue('');
    expect(screen.getByTestId('btn-wiper-start')).toBeDisabled();
  });

  it('explains when the platform cannot wipe whole drives', async () => {
    api.current.handlers['wiper.list_devices'] = () => ({ supported: false, devices: [] });
    const user = await open();
    await user.click(screen.getByTestId('wiper-mode-drive'));
    expect(await screen.findByTestId('wiper-unsupported')).toBeInTheDocument();
    expect(screen.getByTestId('btn-wiper-start')).toBeDisabled();
  });

  it('warns that cancelling a drive wipe is destructive', async () => {
    api.current.handlers['wiper.wipe_drive'] = (_p, opts) => pendingUntilAborted(opts);
    const user = await drive();
    await user.click(screen.getByTestId('wiper-device-/dev/sdc'));
    await user.type(screen.getByTestId('wiper-confirm-input'), '/dev/sdc');
    await user.click(screen.getByTestId('btn-wiper-start'));
    await user.click(screen.getByTestId('confirm-sheet-confirm'));
    expect(await screen.findByTestId('wiper-progress')).toHaveTextContent('partly overwritten');
    await user.click(screen.getByTestId('btn-cancel'));
    await waitFor(() => expect(screen.queryByTestId('wiper-progress')).not.toBeInTheDocument());
  });
});
