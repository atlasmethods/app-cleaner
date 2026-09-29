import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { CookieDomain } from '../api/cookies';
import { createApiMock } from '../test/mockApi';

const api = vi.hoisted(() => ({ current: null as unknown as ReturnType<typeof createApiMock> }));
vi.mock('../lib/transport', async (orig) => {
  const actual = await orig<typeof import('../lib/transport')>();
  return { ...actual, call: (m: string, p?: unknown, o?: object) => api.current.call(m, p, o) };
});

import CookiesPage from './CookiesPage';

const cookie = (domain: string, count: number, browsers = ['Google Chrome']): CookieDomain => ({
  domain,
  count,
  browsers,
  kept: false,
});

const COOKIES = [
  cookie('google.com', 2),
  cookie('accounts.google.com', 1),
  cookie('github.com', 3, ['Google Chrome', 'Mozilla Firefox']),
  cookie('notgoogle.com', 1),
  cookie('tracker.example', 4),
];

function renderPage() {
  return render(
    <MemoryRouter>
      <CookiesPage />
    </MemoryRouter>,
  );
}

const listed = () =>
  screen
    .getAllByTestId(/^cookie-[^-]/)
    .map((e) => e.getAttribute('data-testid')!.replace('cookie-', ''))
    .filter((d) => !d.startsWith('select-') && !d.startsWith('keep-'));

let keepState: string[];

beforeEach(() => {
  api.current = createApiMock();
  keepState = [];
  api.current.handlers['cookies.list'] = () => COOKIES;
  api.current.handlers['cookies.get_keep_list'] = () => ({ domains: keepState });
  api.current.handlers['cookies.set_keep_list'] = (p) => {
    keepState = (p as { domains: string[] }).domains;
    return { domains: keepState };
  };
});

describe('CookiesPage', () => {
  it('shows "Cookies on this computer" and "Cookies to keep"', async () => {
    renderPage();
    expect(await screen.findByTestId('cookies-all')).toHaveTextContent('Cookies on this computer (5)');
    expect(screen.getByTestId('cookies-keep')).toHaveTextContent('Cookies to keep (0)');
    expect(screen.getByTestId('cookies-keep-empty')).toBeInTheDocument();
    expect(screen.getByTestId('cookie-github.com')).toHaveTextContent('3 cookies - Google Chrome, Mozilla Firefox');
    expect(screen.getByTestId('cookie-google.com')).toHaveTextContent('2 cookies');
  });

  it('tapping a site moves it (with its subdomains) to the keep list and saves it', async () => {
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('cookies-all');
    await user.click(screen.getByTestId('cookie-keep-google.com'));
    await waitFor(() => expect(api.current.paramsOf('cookies.set_keep_list')).toHaveLength(1));
    expect(api.current.paramsOf('cookies.set_keep_list')[0]).toEqual({ domains: ['google.com'] });
    expect(screen.queryByTestId('cookie-google.com')).toBeNull();
    expect(screen.queryByTestId('cookie-accounts.google.com')).toBeNull();
    expect(screen.getByTestId('cookie-notgoogle.com')).toBeInTheDocument();
    expect(screen.getByTestId('keep-google.com')).toHaveTextContent('3 cookies protected');
    expect(screen.getByTestId('cookies-all')).toHaveTextContent('(3)');
    expect(screen.getByTestId('cookies-keep')).toHaveTextContent('(1)');
  });

  it('tapping a kept site moves it back', async () => {
    keepState = ['github.com'];
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('keep-github.com');
    expect(screen.queryByTestId('cookie-github.com')).toBeNull();
    await user.click(screen.getByTestId('keep-remove-github.com'));
    await waitFor(() => expect(api.current.paramsOf('cookies.set_keep_list')[0]).toEqual({ domains: [] }));
    expect(screen.getByTestId('cookie-github.com')).toBeInTheDocument();
    expect(screen.queryByTestId('keep-github.com')).toBeNull();
  });

  it('rolls the move back and shows an error when saving fails', async () => {
    const { ApiCallError } = await import('../lib/transport');
    api.current.handlers['cookies.set_keep_list'] = () => Promise.reject(new ApiCallError('InvalidParams', 'nope'));
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('cookies-all');
    await user.click(screen.getByTestId('cookie-keep-github.com'));
    expect(await screen.findByTestId('error-banner')).toHaveTextContent('nope');
    expect(screen.getByTestId('cookie-github.com')).toBeInTheDocument();
    expect(screen.queryByTestId('keep-github.com')).toBeNull();
  });

  it('search filters both lists', async () => {
    keepState = ['github.com'];
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('cookies-all');
    await user.type(screen.getByTestId('cookies-search'), 'GOOGLE');
    expect(listed().sort()).toEqual(['accounts.google.com', 'google.com', 'notgoogle.com']);
    expect(screen.queryByTestId('keep-github.com')).toBeNull();
    await user.clear(screen.getByTestId('cookies-search'));
    await user.type(screen.getByTestId('cookies-search'), 'git');
    expect(screen.getByTestId('keep-github.com')).toBeInTheDocument();
    expect(screen.getByTestId('cookies-all-empty')).toBeInTheDocument();
  });

  it('Smart keep runs the intelligent scan and reports what it added', async () => {
    api.current.handlers['cookies.intelligent_scan'] = () => {
      keepState = ['google.com', 'github.com'];
      return { domains: keepState, added: ['google.com', 'github.com'] };
    };
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('cookies-all');
    await user.click(screen.getByTestId('btn-smart-keep'));
    await waitFor(() => expect(screen.getByTestId('cookies-note')).toHaveTextContent('Keeping 2 well-known sites'));
    expect(screen.getByTestId('keep-google.com')).toBeInTheDocument();
    expect(screen.getByTestId('keep-github.com')).toBeInTheDocument();
    expect(screen.getByTestId('cookies-all')).toHaveTextContent('(2)');
    expect(listed()).toEqual(['notgoogle.com', 'tracker.example']);
  });

  it('multi-select delete confirms, sends the selected domains and reloads', async () => {
    let n = 0;
    api.current.handlers['cookies.list'] = () => (n++ === 0 ? COOKIES : COOKIES.filter((c) => c.domain !== 'tracker.example'));
    api.current.handlers['cookies.delete'] = () => ({
      deleted: 4,
      browsers: [{ browser: 'Google Chrome', deleted: 4, errors: [] }],
    });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('cookies-all');
    expect(screen.queryByTestId('btn-cookies-delete')).toBeNull();
    await user.click(screen.getByTestId('cookie-select-tracker.example'));
    expect(screen.getByTestId('btn-cookies-delete')).toHaveTextContent('Delete selected (1)');
    await user.click(screen.getByTestId('btn-cookies-delete'));
    const sheet = await screen.findByTestId('confirm-sheet');
    expect(sheet).toHaveTextContent('Delete cookies of 1 site?');
    expect(api.current.paramsOf('cookies.delete')).toHaveLength(0);
    await user.click(within(sheet).getByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('cookies.delete')).toHaveLength(1));
    expect(api.current.paramsOf('cookies.delete')[0]).toEqual({ domains: ['tracker.example'] });
    await waitFor(() => expect(screen.queryByTestId('cookie-tracker.example')).toBeNull());
    expect(screen.getByTestId('cookies-note')).toHaveTextContent('Deleted 4 cookies.');
    expect(screen.queryByTestId('btn-cookies-delete')).toBeNull();
  });

  it('selected sites that get hidden by the search are not deleted', async () => {
    api.current.handlers['cookies.delete'] = () => ({ deleted: 1, browsers: [] });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('cookies-all');
    await user.click(screen.getByTestId('cookie-select-tracker.example'));
    await user.click(screen.getByTestId('cookie-select-github.com'));
    await user.type(screen.getByTestId('cookies-search'), 'github');
    expect(screen.getByTestId('btn-cookies-delete')).toHaveTextContent('Delete selected (1)');
    await user.click(screen.getByTestId('btn-cookies-delete'));
    await user.click(await screen.findByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(api.current.paramsOf('cookies.delete')).toHaveLength(1));
    expect(api.current.paramsOf('cookies.delete')[0]).toEqual({ domains: ['github.com'] });
  });

  it('reports browsers that were skipped because they are running', async () => {
    api.current.handlers['cookies.delete'] = () => ({
      deleted: 0,
      browsers: [{ browser: 'Google Chrome', deleted: 0, skipped: 'app_running', errors: [] }],
    });
    renderPage();
    const user = userEvent.setup();
    await screen.findByTestId('cookies-all');
    await user.click(screen.getByTestId('cookie-select-github.com'));
    await user.click(screen.getByTestId('btn-cookies-delete'));
    await user.click(await screen.findByTestId('confirm-sheet-confirm'));
    await waitFor(() => expect(screen.getByTestId('cookies-note')).toHaveTextContent('Skipped Google Chrome (running)'));
  });

  it('shows an empty state when there are no cookies', async () => {
    api.current.handlers['cookies.list'] = () => [];
    renderPage();
    expect(await screen.findByTestId('cookies-all-empty')).toHaveTextContent('No cookies found');
  });
});
