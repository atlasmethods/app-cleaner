import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { BottomSheet } from './BottomSheet';
import { ConfirmSheet } from './ConfirmSheet';

function Harness({ nested = false }: { nested?: boolean }) {
  const [open, setOpen] = useState(false);
  const [confirm, setConfirm] = useState(false);
  return (
    <div>
      <main data-scroll-root>
        <button onClick={() => setOpen(true)}>opener</button>
      </main>
      <BottomSheet open={open} title="Menu" onClose={() => setOpen(false)}>
        <button>first</button>
        <button onClick={() => setConfirm(true)}>second</button>
      </BottomSheet>
      {nested && (
        <ConfirmSheet open={confirm} title="Sure?" onConfirm={() => setConfirm(false)} onCancel={() => setConfirm(false)} />
      )}
    </div>
  );
}

describe('BottomSheet', () => {
  it('moves focus in, traps Tab, closes on Escape, restores focus and unlocks scrolling', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const opener = screen.getByText('opener');
    await user.click(opener);
    const dialog = screen.getByRole('dialog', { name: 'Menu' });
    expect(dialog).toContainElement(document.activeElement as HTMLElement);
    expect(document.activeElement).toHaveTextContent('first');
    expect(document.querySelector<HTMLElement>('[data-scroll-root]')!.style.overflowY).toBe('hidden');
    await user.tab();
    expect(document.activeElement).toHaveTextContent('second');
    await user.tab(); // wraps around instead of reaching the page behind
    expect(document.activeElement).toHaveTextContent('first');
    await user.tab({ shift: true });
    expect(document.activeElement).toHaveTextContent('second');
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(opener).toHaveFocus();
    expect(document.querySelector<HTMLElement>('[data-scroll-root]')!.style.overflowY).toBe('');
  });

  it('closes on a backdrop tap', async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByText('opener'));
    await user.click(screen.getByTestId('bottom-sheet-close'));
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('Escape only closes the topmost of two stacked sheets', async () => {
    const user = userEvent.setup();
    render(<Harness nested />);
    await user.click(screen.getByText('opener'));
    await user.click(screen.getByText('second'));
    expect(screen.getAllByRole('dialog')).toHaveLength(2);
    await user.keyboard('{Escape}');
    expect(screen.getAllByRole('dialog')).toHaveLength(1);
    expect(screen.getByRole('dialog', { name: 'Menu' })).toBeInTheDocument();
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.querySelector<HTMLElement>('[data-scroll-root]')!.style.overflowY).toBe('');
  });
});

describe('ConfirmSheet', () => {
  it('starts on Cancel, and Escape cancels', async () => {
    const user = userEvent.setup();
    const onCancel = vi.fn();
    render(<ConfirmSheet open title="Delete?" message="Gone for good" danger onConfirm={vi.fn()} onCancel={onCancel} />);
    expect(screen.getByRole('button', { name: 'Cancel' })).toHaveFocus();
    expect(screen.getByRole('dialog')).toHaveAccessibleDescription('Gone for good');
    await user.keyboard('{Escape}');
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it('a strong confirmation needs the exact word and starts empty every time', async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    const { rerender } = render(<ConfirmSheet open title="T" requireText="SHRED" onConfirm={onConfirm} onCancel={vi.fn()} />);
    const confirm = screen.getByTestId('confirm-sheet-confirm');
    expect(confirm).toBeDisabled();
    await user.type(screen.getByLabelText(/Type SHRED/), 'shred');
    expect(confirm).toBeDisabled();
    await user.clear(screen.getByTestId('confirm-sheet-input'));
    await user.type(screen.getByTestId('confirm-sheet-input'), 'SHRED');
    expect(confirm).toBeEnabled();
    await user.click(confirm);
    expect(onConfirm).toHaveBeenCalledTimes(1);
    rerender(<ConfirmSheet open={false} title="T" requireText="SHRED" onConfirm={onConfirm} onCancel={vi.fn()} />);
    rerender(<ConfirmSheet open title="T" requireText="SHRED" onConfirm={onConfirm} onCancel={vi.fn()} />);
    expect(screen.getByTestId('confirm-sheet-input')).toHaveValue('');
    expect(screen.getByTestId('confirm-sheet-confirm')).toBeDisabled();
  });
});
