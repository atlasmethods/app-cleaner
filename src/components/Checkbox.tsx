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

/** Native checkbox with tri-state support and a comfortable touch target. */
export function Checkbox({ checked, indeterminate = false, onChange, ariaLabel, testId, disabled, id }: Props) {
  const ref = useRef<HTMLInputElement>(null);
  const latest = useRef(indeterminate);
  useLayoutEffect(() => {
    latest.current = indeterminate;
    if (ref.current) ref.current.indeterminate = indeterminate;
  }); // every render: a click clears the DOM flag even when the parent keeps the same state
  return (
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
      className="m-0 h-5 w-5 shrink-0 cursor-pointer accent-accent disabled:cursor-not-allowed"
    />
  );
}
