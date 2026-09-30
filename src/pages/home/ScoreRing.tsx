import { ringAriaLabel, scoreTone, type Tone } from '../../lib/health';

const STROKE: Record<Tone, string> = {
  ok: 'stroke-ok',
  warn: 'stroke-warn',
  danger: 'stroke-danger',
  muted: 'stroke-muted',
};

const SIZE = 132;
const WIDTH = 12;
const RADIUS = (SIZE - WIDTH) / 2;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

interface Props {
  score: number | null;
  /** Dim the ring: it shows the previous scan, not a fresh one. */
  stale?: boolean;
}

/** Circular gauge of the 0..100 health score. The label is on the image, the number is decoration. */
export function ScoreRing({ score, stale = false }: Props) {
  const tone = scoreTone(score);
  const pct = score === null ? 0 : Math.min(100, Math.max(0, score));
  return (
    <div className={`relative shrink-0 ${stale ? 'opacity-70' : ''}`} style={{ width: SIZE, height: SIZE }}>
      <svg
        role="img"
        aria-label={ringAriaLabel(score)}
        data-testid="health-ring"
        viewBox={`0 0 ${SIZE} ${SIZE}`}
        width={SIZE}
        height={SIZE}
        className="block"
      >
        <circle cx={SIZE / 2} cy={SIZE / 2} r={RADIUS} fill="none" strokeWidth={WIDTH} className="stroke-surface-2" />
        <circle
          cx={SIZE / 2}
          cy={SIZE / 2}
          r={RADIUS}
          fill="none"
          strokeWidth={WIDTH}
          strokeLinecap="round"
          strokeDasharray={CIRCUMFERENCE}
          strokeDashoffset={CIRCUMFERENCE * (1 - pct / 100)}
          transform={`rotate(-90 ${SIZE / 2} ${SIZE / 2})`}
          className={STROKE[tone]}
        />
      </svg>
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center"
        data-testid="health-score"
      >
        <span className="text-4xl font-bold leading-none">{score === null ? '--' : score}</span>
        <span className="mt-1 text-[11px] text-muted">of 100</span>
      </div>
    </div>
  );
}
