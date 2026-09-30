import type { ReactNode } from 'react';
import { Switch } from '../../components/Switch';

interface Props {
  label: string;
  hint?: ReactNode;
  checked: boolean;
  onChange: (next: boolean) => void;
  testId: string;
  disabled?: boolean;
}

/** A settings row: text on the left, switch on the right (wraps safely at 320 px). */
export function SwitchRow({ label, hint, checked, onChange, testId, disabled }: Props) {
  return (
    <div className="flex min-w-0 items-center gap-2">
      <div className="min-w-0 flex-1 text-sm">
        <span className="block break-words font-medium">{label}</span>
        {hint && <span className="block break-words text-xs text-muted">{hint}</span>}
      </div>
      <Switch checked={checked} onChange={onChange} ariaLabel={label} testId={testId} disabled={disabled} />
    </div>
  );
}
