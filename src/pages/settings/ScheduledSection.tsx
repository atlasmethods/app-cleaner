import { CalendarClock, ChevronRight } from 'lucide-react';
import { Link } from 'react-router-dom';
import { Card } from '../../components/Card';
import { t } from '../../i18n';

/** Entry point to the schedules page. */
export function ScheduledSection() {
  return (
    <Card title={t('settings.schedules')} testId="settings-schedules">
      <Link
        to="/settings/schedules"
        data-testid="settings-schedules-link"
        className="flex min-h-11 min-w-0 items-center gap-3 rounded-xl text-fg no-underline"
      >
        <CalendarClock size={20} className="shrink-0 text-accent" aria-hidden />
        <span className="min-w-0 flex-1 text-sm">
          <span className="block font-medium">Manage schedules</span>
          <span className="block break-words text-xs text-muted">Clean automatically every day, week or month.</span>
        </span>
        <ChevronRight size={18} className="shrink-0 text-muted" aria-hidden />
      </Link>
    </Card>
  );
}
