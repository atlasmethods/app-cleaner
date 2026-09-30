import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { _resetAuthForTests } from '../lib/transport';
import { TOOLS } from '../nav';
import AuthRequiredPage from '../pages/AuthRequiredPage';
import NotFoundPage from '../pages/NotFoundPage';
import { ErrorBanner } from './ErrorBanner';
import { NavRail } from './NavRail';
import { Checkbox } from './Checkbox';
import { ApiCallError } from '../lib/transport';

describe('NavRail', () => {
  it('lists the five tabs and every tool, marking the current one', () => {
    render(
      <MemoryRouter initialEntries={['/tools/shredder']}>
        <NavRail />
      </MemoryRouter>,
    );
    const nav = screen.getByRole('navigation', { name: 'Main' });
    expect(within(nav).getByTestId('tab-tools')).toHaveAttribute('aria-current', 'page');
    const tools = within(screen.getByRole('list', { name: 'Tools' })).getAllByRole('link');
    expect(tools).toHaveLength(TOOLS.length);
    expect(TOOLS.map((t) => t.id)).toContain('shredder');
    expect(screen.getByTestId('rail-shredder')).toHaveAttribute('aria-current', 'page');
    expect(screen.getByTestId('rail-wiper')).not.toHaveAttribute('aria-current');
  });
});

describe('NotFoundPage', () => {
  it('names the missing route and links home', async () => {
    const user = userEvent.setup();
    render(
      <MemoryRouter initialEntries={['/tools/nope']}>
        <Routes>
          <Route path="/" element={<p>home</p>} />
          <Route path="*" element={<NotFoundPage />} />
        </Routes>
      </MemoryRouter>,
    );
    expect(screen.getByTestId('page-not-found')).toHaveTextContent('/tools/nope');
    await user.click(screen.getByTestId('not-found-home'));
    expect(screen.getByText('home')).toBeInTheDocument();
  });
});

describe('AuthRequiredPage', () => {
  beforeEach(() => _resetAuthForTests());
  it('tells the user to open the link printed by clearsweep ui', () => {
    render(<AuthRequiredPage />);
    expect(screen.getByRole('heading')).toHaveTextContent('Open ClearSweep from its link');
    expect(screen.getByTestId('page-auth-required')).toHaveTextContent('clearsweep ui');
  });
});

describe('ErrorBanner', () => {
  const reload = vi.fn();
  beforeEach(() => {
    vi.stubGlobal('location', { ...window.location, reload });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    reload.mockReset();
  });

  it('says the backend is unreachable and retries by reloading when the page gives no retry', async () => {
    const user = userEvent.setup();
    render(<ErrorBanner error={new ApiCallError('Unreachable', 'service is down')} />);
    expect(screen.getByRole('alert')).toHaveTextContent('Backend unreachable');
    expect(screen.getByRole('alert')).toHaveTextContent('service is down');
    await user.click(screen.getByRole('button', { name: 'Retry' }));
    expect(reload).toHaveBeenCalled();
  });

  it('uses the page retry when there is one, and offers none for ordinary errors without it', async () => {
    const user = userEvent.setup();
    const retry = vi.fn();
    const { rerender } = render(<ErrorBanner error={new ApiCallError('Io', 'disk')} onRetry={retry} />);
    await user.click(screen.getByRole('button', { name: 'Retry' }));
    expect(retry).toHaveBeenCalled();
    rerender(<ErrorBanner error={new ApiCallError('Io', 'disk')} />);
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
  });
});

describe('Checkbox', () => {
  it('is a real checkbox with a 40px touch area', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Checkbox checked={false} onChange={onChange} ariaLabel="pick" />);
    const box = screen.getByRole('checkbox', { name: 'pick' });
    expect(box.className).toContain('absolute inset-0');
    expect(box.parentElement!.className).toContain('h-10 w-10');
    await user.click(box);
    expect(onChange).toHaveBeenCalledWith(true);
  });

  it('exposes the mixed state', () => {
    render(<Checkbox checked={false} indeterminate onChange={() => undefined} ariaLabel="some" />);
    const box = screen.getByRole('checkbox', { name: 'some' });
    expect(box).toHaveAttribute('aria-checked', 'mixed');
    expect((box as HTMLInputElement).indeterminate).toBe(true);
  });
});
