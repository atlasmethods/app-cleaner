import type { ReactNode } from 'react';
import type { HealthCategory, HealthReport } from '../../api/health';
import { BottomSheet } from '../../components/BottomSheet';
import { Checkbox } from '../../components/Checkbox';
import { formatBytes } from '../../lib/format';
import {
  backgroundApps,
  canFix,
  historyCount,
  privacyBrowsers,
  startupItems,
  toggleIn,
  trackerCount,
  updateItems,
  type Selection,
} from '../../lib/health';
import { versionChange } from '../../lib/updates';

interface Props {
  /** The category to show; `null` closes the sheet. */
  category: HealthCategory | null;
  report: HealthReport;
  selection: Selection;
  onChange: (next: Selection) => void;
  onClose: () => void;
  /** The report is from a previous scan: nothing can be selected until the next scan. */
  readOnly: boolean;
}

function Row({
  testId,
  checked,
  onChange,
  disabled,
  title,
  sub,
  badge,
}: {
  testId: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled: boolean;
  title: string;
  sub?: string;
  badge?: ReactNode;
}) {
  return (
    <label className="flex min-w-0 items-start gap-2 rounded-lg border border-line bg-surface-2 p-2">
      <span className="pt-0.5">
        <Checkbox checked={checked} onChange={onChange} disabled={disabled} ariaLabel={title} testId={testId} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block break-words text-sm font-medium">{title}</span>
        {sub && <span className="block break-words text-xs text-muted">{sub}</span>}
      </span>
      {badge}
    </label>
  );
}

function Badge({ children, tone }: { children: ReactNode; tone: 'warn' | 'danger' | 'muted' }) {
  const cls = tone === 'danger' ? 'text-danger' : tone === 'warn' ? 'text-warn' : 'text-muted';
  return <span className={`shrink-0 text-[11px] font-semibold ${cls}`}>{children}</span>;
}

function Heading({ children }: { children: ReactNode }) {
  return <h3 className="m-0 mt-1 text-xs font-semibold uppercase tracking-wide text-muted">{children}</h3>;
}

function Note({ children, testId }: { children: ReactNode; testId?: string }) {
  return (
    <p className="m-0 break-words text-xs text-muted" data-testid={testId}>
      {children}
    </p>
  );
}

function join(list: string[]): string {
  return list.join(', ');
}

function Body({ category, report, selection, onChange, readOnly }: Omit<Props, 'onClose' | 'category'> & { category: HealthCategory }) {
  if (category.status === 'unavailable') {
    return <Note testId="sheet-unavailable">{category.summary}</Note>;
  }
  if (!canFix(category)) {
    return <Note testId="sheet-nothing">{category.summary}</Note>;
  }
  switch (category.id) {
    case 'privacy': {
      const trackers = trackerCount(category);
      const history = historyCount(category);
      const browsers = privacyBrowsers(category);
      return (
        <>
          <Row
            testId="sel-privacy"
            checked={selection.privacy}
            disabled={readOnly}
            onChange={(v) => onChange({ ...selection, privacy: v })}
            title="Clean tracking data"
            sub={[
              trackers > 0 ? `${trackers} tracking ${trackers === 1 ? 'cookie' : 'cookies'}` : '',
              history > 0 ? `${history} history ${history === 1 ? 'entry' : 'entries'}` : '',
            ]
              .filter(Boolean)
              .join(', ')}
          />
          {browsers.length > 0 && <Note>In {join(browsers)}.</Note>}
          <Note>Cookies of sites on your keep list are never removed.</Note>
        </>
      );
    }
    case 'space': {
      const bytes = category.metrics.bytes ?? 0;
      const files = category.metrics.files ?? 0;
      return (
        <>
          <Row
            testId="sel-space"
            checked={selection.space}
            disabled={readOnly}
            onChange={(v) => onChange({ ...selection, space: v })}
            title="Delete junk files"
            sub={`${formatBytes(bytes)} in ${files} ${files === 1 ? 'file' : 'files'}`}
          />
          <Heading>Biggest areas</Heading>
          <ul className="m-0 flex list-none flex-col gap-1 p-0" data-testid="sheet-junk">
            {category.findings.map((f) =>
              f.kind === 'junk' ? (
                <li key={f.group} className="flex justify-between gap-2 text-sm">
                  <span className="min-w-0 break-words">{f.group}</span>
                  <span className="shrink-0 text-muted">{formatBytes(f.bytes)}</span>
                </li>
              ) : null,
            )}
          </ul>
          <Note>Uses the items ticked on the Clean tab.</Note>
        </>
      );
    }
    case 'speed': {
      const items = startupItems(report);
      const apps = backgroundApps(report);
      return (
        <>
          {items.length > 0 && (
            <>
              <Heading>Startup items to switch off</Heading>
              {items.map((i) => (
                <Row
                  key={i.id}
                  testId={`sel-startup-${i.id}`}
                  checked={selection.startup.has(i.id)}
                  disabled={readOnly}
                  onChange={() => onChange({ ...selection, startup: toggleIn(selection.startup, i.id) })}
                  title={i.name}
                  badge={<Badge tone={i.impact === 'high' ? 'danger' : 'warn'}>{i.impact === 'high' ? 'High impact' : 'Medium impact'}</Badge>}
                />
              ))}
            </>
          )}
          {apps.length > 0 && (
            <>
              <Heading>Apps to put to sleep</Heading>
              {apps.map((a) => (
                <Row
                  key={a.appId}
                  testId={`sel-app-${a.appId}`}
                  checked={selection.apps.has(a.appId)}
                  disabled={readOnly}
                  onChange={() => onChange({ ...selection, apps: toggleIn(selection.apps, a.appId) })}
                  title={a.name}
                  sub={`${formatBytes(a.memoryBytes)} in the background`}
                />
              ))}
              <Note>Sleeping apps are asked to quit and stop starting automatically. Wake them from the Performance tab.</Note>
            </>
          )}
        </>
      );
    }
    case 'security': {
      const items = updateItems(report);
      const all = new Set(items.map((u) => u.id));
      const securityOnly = new Set(items.filter((u) => u.security).map((u) => u.id));
      return (
        <>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              disabled={readOnly}
              onClick={() => onChange({ ...selection, updates: securityOnly })}
              data-testid="updates-security-only"
              className="h-10 rounded-lg border border-line bg-surface-2 px-2 text-xs font-medium disabled:opacity-50"
            >
              Security only
            </button>
            <button
              type="button"
              disabled={readOnly}
              onClick={() => onChange({ ...selection, updates: all })}
              data-testid="updates-all"
              className="h-10 rounded-lg border border-line bg-surface-2 px-2 text-xs font-medium disabled:opacity-50"
            >
              All
            </button>
            <button
              type="button"
              disabled={readOnly}
              onClick={() => onChange({ ...selection, updates: new Set() })}
              data-testid="updates-none"
              className="h-10 rounded-lg border border-line bg-surface-2 px-2 text-xs font-medium disabled:opacity-50"
            >
              None
            </button>
          </div>
          {items.map((u) => (
            <Row
              key={u.id}
              testId={`sel-update-${u.id}`}
              checked={selection.updates.has(u.id)}
              disabled={readOnly}
              onChange={() => onChange({ ...selection, updates: toggleIn(selection.updates, u.id) })}
              title={u.name}
              sub={versionChange(u)}
              badge={u.security ? <Badge tone="danger">Security</Badge> : undefined}
            />
          ))}
          <Note>Security updates are ticked for you. Other updates are your choice.</Note>
        </>
      );
    }
  }
}

/** Findings of one category, with the checkboxes that decide what "Fix all" does. */
export function FindingsSheet({ category, report, selection, onChange, onClose, readOnly }: Props) {
  return (
    <BottomSheet
      open={category !== null}
      title={category?.title ?? ''}
      subtitle={category?.summary}
      onClose={onClose}
      testId="health-sheet"
    >
      {category && (
        <>
          {readOnly && canFix(category) && (
            <Note testId="sheet-stale">From your last scan. Rescan to change what gets fixed.</Note>
          )}
          <Body category={category} report={report} selection={selection} onChange={onChange} readOnly={readOnly} />
          <Note>Nothing changes until you press Fix all.</Note>
          <button
            type="button"
            onClick={onClose}
            data-testid="health-sheet-done"
            className="mt-1 h-10 w-full rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg"
          >
            Done
          </button>
        </>
      )}
    </BottomSheet>
  );
}
