import { Check, Minus } from 'lucide-react';
import { useLayoutEffect, useRef } from 'react';

interface Props {
  checked: boolean;
  /** Shown for "some but not all" (group checkboxes). Takes visual priority over `checked`. */
  indeterminate?: boolean;
  onChange: (next: boolean) => void;
  ariaLabel?: string;
  testId?: string;
  disabled?: boolean;
  id?: string;
}

/**
 * Native checkbox (keyboard, screen readers, forms) with a drawn box and a 40x40 touch target.
 * The transparent input fills the hit area; the negative margin keeps the layout at 20x20.
 */
export function Checkbox({ checked, indeterminate = false, onChange, ariaLabel, testId, disabled, id }: Props) {
  const ref = useRef<HTMLInputElement>(null);
  const latest = useRef(indeterminate);
  useLayoutEffect(() => {
    latest.current = indeterminate;
    if (ref.current) ref.current.indeterminate = indeterminate;
  }); // every render: a click clears the DOM flag even when the parent keeps the same state
  const filled = checked || indeterminate;
  return (
    <span className="relative -m-2.5 inline-flex h-10 w-10 shrink-0 items-center justify-center">
      <input
        ref={ref}
        id={id}
        type="checkbox"
        checked={checked}
        disabled={disabled}
        aria-label={ariaLabel}
        aria-checked={indeterminate ? 'mixed' : checked}
        data-testid={testId}
        onChange={(e) => {
          const el = e.target;
          onChange(el.checked);
          // The browser drops "indeterminate" on click; restore whatever the parent decided.
          setTimeout(() => {
            el.indeterminate = latest.current;
          }, 0);
        }}
        className="peer absolute inset-0 m-0 h-full w-full cursor-pointer opacity-0 disabled:cursor-not-allowed"
      />
      <span
        aria-hidden
        className={`pointer-events-none flex h-5 w-5 items-center justify-center rounded-[5px] border-2 peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-accent peer-disabled:opacity-50 ${
          filled ? 'border-accent bg-accent text-accent-fg' : 'border-muted bg-surface'
        }`}
      >
        {indeterminate ? <Minus size={14} strokeWidth={3} /> : checked ? <Check size={14} strokeWidth={3} /> : null}
      </span>
    </span>
  );
}
