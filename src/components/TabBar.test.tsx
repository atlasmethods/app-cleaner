import { render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { TabBar } from './TabBar';

describe('TabBar', () => {
  it('renders the 5 tabs in order', () => {
    render(
      <MemoryRouter>
        <TabBar />
      </MemoryRouter>,
    );
    const nav = screen.getByRole('navigation', { name: 'Main' });
    const links = within(nav).getAllByRole('link');
    expect(links).toHaveLength(5);
    expect(links.map((l) => l.getAttribute('data-testid'))).toEqual([
      'tab-home',
      'tab-clean',
      'tab-tools',
      'tab-performance',
      'tab-settings',
    ]);
    expect(links.map((l) => l.textContent)).toEqual(['Home', 'Clean', 'Tools', 'Performance', 'Settings']);
  });

  it('marks the active tab, including for drill-in routes', () => {
    render(
      <MemoryRouter initialEntries={['/tools/sysinfo']}>
        <TabBar />
      </MemoryRouter>,
    );
    expect(screen.getByTestId('tab-tools')).toHaveAttribute('aria-current', 'page');
    expect(screen.getByTestId('tab-home')).not.toHaveAttribute('aria-current');
  });
});
