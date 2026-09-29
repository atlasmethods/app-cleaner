interface Props {
  /** 0..100 */
  value: number;
  label?: string;
  /** Turns the bar red/amber as it fills (disk / memory usage). */
  tone?: 'accent' | 'usage';
}

export function ProgressBar({ value, label, tone = 'accent' }: Props) {
  const v = Math.min(100, Math.max(0, value));
  const color =
    tone === 'usage' && v >= 90 ? 'bg-danger' : tone === 'usage' && v >= 75 ? 'bg-warn' : 'bg-accent';
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(v)}
      className="h-2 w-full overflow-hidden rounded-full bg-surface-2"
    >
      <div className={`h-full rounded-full ${color}`} style={{ width: `${v}%` }} />
    </div>
  );
}
