import {
  ChevronRight,
  CircleAlert,
  CircleCheck,
  CircleSlash,
  Cookie,
  Gauge,
  HardDrive,
  ShieldCheck,
  TriangleAlert,
  type LucideIcon,
} from 'lucide-react';
import type { CategoryId, HealthCategory, HealthStatus } from '../../api/health';
import { STATUS_LABEL, statusTone, type Tone } from '../../lib/health';

const CATEGORY_ICON: Record<CategoryId, LucideIcon> = {
  privacy: Cookie,
  space: HardDrive,
  speed: Gauge,
  security: ShieldCheck,
};

const STATUS_ICON: Record<HealthStatus, LucideIcon> = {
  good: CircleCheck,
  warning: TriangleAlert,
  problem: CircleAlert,
  unavailable: CircleSlash,
};

const TONE_TEXT: Record<Tone, string> = {
  ok: 'text-ok',
  warn: 'text-warn',
  danger: 'text-danger',
  muted: 'text-muted',
};

interface Props {
  category: HealthCategory;
  onOpen: () => void;
  /** Shown from the previous scan. */
  stale?: boolean;
}

/** One category: icon, status (colour + icon + word), one-line summary. Tap for details. */
export function CategoryCard({ category, onOpen, stale = false }: Props) {
  const Icon = CATEGORY_ICON[category.id];
  const StatusIcon = STATUS_ICON[category.status];
  const tone = TONE_TEXT[statusTone(category.status)];
  return (
    <button
      type="button"
      onClick={onOpen}
      data-testid={`cat-${category.id}`}
      data-status={category.status}
      aria-label={`${category.title}: ${STATUS_LABEL[category.status]}. ${category.summary}`}
      className={`flex w-full min-w-0 items-center gap-3 rounded-xl border border-line bg-surface p-3 text-left ${
        stale ? 'opacity-75' : ''
      }`}
    >
      <Icon size={22} className="shrink-0 text-accent" aria-hidden />
      <span className="min-w-0 flex-1">
        <span className="flex flex-wrap items-center gap-x-1.5">
          <span className="text-sm font-semibold">{category.title}</span>
          <span
            className={`flex items-center gap-1 text-[11px] font-medium ${tone}`}
            data-testid={`cat-${category.id}-status`}
          >
            <StatusIcon size={13} aria-hidden />
            {STATUS_LABEL[category.status]}
          </span>
        </span>
        <span className="mt-0.5 block break-words text-xs text-muted" data-testid={`cat-${category.id}-summary`}>
          {category.summary}
        </span>
      </span>
      <ChevronRight size={16} className="shrink-0 text-muted" aria-hidden />
    </button>
  );
}
