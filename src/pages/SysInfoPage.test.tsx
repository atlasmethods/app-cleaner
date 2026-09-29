import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SysInfo } from '../api/sysinfo';

const sample: SysInfo = {
  os: { name: 'Ubuntu', version: '24.04', kernel: '6.1', hostname: 'box', arch: 'x86_64' },
  cpu: { brand: 'Test CPU 9000', cores: 8, usage: 12.5 },
  memory: { total: 16 * 1024 ** 3, used: 4 * 1024 ** 3 },
  disks: [{ name: '/dev/sda1', mount: '/', total: 100 * 1024 ** 3, available: 40 * 1024 ** 3, fs: 'ext4' }],
  uptimeSecs: 3700,
};

// A plain function (not vi.fn) so a rejecting mock does not leave a tracked promise unhandled.
const calls: string[] = [];
let respond: () => Promise<unknown> = () => Promise.resolve(sample);
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return {
    ...actual,
    call: (method: string) => {
      calls.push(method);
      return respond();
    },
  };
});

import SysInfoPage from './SysInfoPage';

describe('SysInfoPage', () => {
  beforeEach(() => {
    calls.length = 0;
    respond = () => Promise.resolve(sample);
  });

  it('renders CPU, memory and disk usage from sysinfo.get', async () => {
    render(
      <MemoryRouter>
        <SysInfoPage />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByTestId('sysinfo-cpu')).toBeInTheDocument());
    expect(calls[0]).toBe('sysinfo.get');
    expect(screen.getByTestId('sysinfo-cpu-brand')).toHaveTextContent('Test CPU 9000');
    expect(screen.getByTestId('sysinfo-memory')).toHaveTextContent('4.0 GB of 16.0 GB used');
    const bars = screen.getAllByRole('progressbar');
    const disk = bars.find((b) => b.getAttribute('aria-label') === 'Disk usage /');
    expect(disk).toHaveAttribute('aria-valuenow', '60');
  });

  it('shows an error banner when the call fails', async () => {
    const { ApiCallError } = await import('../lib/transport');
    respond = () => Promise.reject(new ApiCallError('PermissionDenied', 'nope'));
    render(
      <MemoryRouter>
        <SysInfoPage />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByTestId('error-banner')).toHaveTextContent('nope'));
  });
});
