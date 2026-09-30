import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ApiCallError } from '../lib/transport';
import { ErrorBanner } from './ErrorBanner';

describe('ErrorBanner', () => {
  it('renders nothing without an error', () => {
    const { container } = render(<ErrorBanner error={null} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('shows real failures as a red alert with the error code', () => {
    render(<ErrorBanner error={new ApiCallError('Io', 'disk on fire')} />);
    const b = screen.getByRole('alert');
    expect(b).toHaveAttribute('data-testid', 'error-banner');
    expect(b).toHaveTextContent('Io');
    expect(b).toHaveTextContent('disk on fire');
  });

  it('shows Unsupported as a neutral notice: no alert role, no code, no error colours', () => {
    render(<ErrorBanner error={new ApiCallError('Unsupported', 'Repair is only available on Windows')} />);
    expect(screen.queryByRole('alert')).toBeNull();
    const b = screen.getByTestId('info-banner');
    expect(b).toHaveAttribute('role', 'status');
    expect(b).toHaveTextContent('Repair is only available on Windows');
    expect(b).not.toHaveTextContent('Unsupported');
    expect(b.className).not.toMatch(/danger/);
    expect(screen.queryByTestId('error-banner')).toBeNull();
  });
});
