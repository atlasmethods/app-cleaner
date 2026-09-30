import { useEffect, useMemo, useState } from 'react';
import type { RulesListing } from '../../api/cleaner';
import type { AgentStatus } from '../../api/smart_cleaning';
import { Card } from '../../components/Card';
import { Checkbox } from '../../components/Checkbox';
import { t } from '../../i18n';
import { formatWhen } from '../../lib/format';
import { SMART_THRESHOLD_PRESETS, agentLine, slug } from '../../lib/schedule';
import { call } from '../../lib/transport';
import { SwitchRow } from './SwitchRow';
import type { SectionProps } from './types';

const INTERVALS = [5, 15, 30, 60, 120, 360, 720, 1440];

const selectClass = 'h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 px-2 text-sm';

function intervalLabel(min: number): string {
  if (min < 60) return `Every ${min} minutes`;
  if (min === 60) return 'Every hour';
  if (min % 60 === 0 && min < 1440) return `Every ${min / 60} hours`;
  if (min === 1440) return 'Once a day';
  return `Every ${min} minutes`;
}

export function SmartCleaningSection({ settings, patch }: SectionProps) {
  const smart = settings.smart;
  const [browsers, setBrowsers] = useState<string[]>([]);
  const [status, setStatus] = useState<AgentStatus | null>(null);
  const [custom, setCustom] = useState(String(smart.thresholdMb));
  const [customError, setCustomError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    // Both are extras: when they cannot be loaded the section still works.
    call<RulesListing>('cleaner.list_rules')
      .then((l) => {
        if (!live) return;
        const cat = l.categories.find((c) => c.category === 'browser');
        setBrowsers(cat ? cat.groups.map((g) => g.group) : []);
      })
      .catch(() => undefined);
    call<AgentStatus>('smart_cleaning.status')
      .then((s) => live && setStatus(s))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, []);

  const intervals = useMemo(() => {
    const cur = smart.checkIntervalMinutes ?? 60;
    return INTERVALS.includes(cur) ? INTERVALS : [...INTERVALS, cur].sort((a, b) => a - b);
  }, [smart.checkIntervalMinutes]);

  const setSmart = (p: Partial<typeof smart>) => void patch({ smart: p });

  const pickPreset = (mb: number) => {
    setCustom(String(mb));
    setCustomError(null);
    if (mb !== smart.thresholdMb) setSmart({ thresholdMb: mb });
  };

  const commitCustom = () => {
    const n = Number(custom);
    if (!Number.isInteger(n) || n < 1 || n > 10_000_000) {
      setCustomError('Enter a whole number of megabytes, 1 or more.');
      return;
    }
    setCustomError(null);
    if (n !== smart.thresholdMb) setSmart({ thresholdMb: n });
  };

  const toggleBrowser = (group: string, on: boolean) => {
    const cur = smart.cleanOnBrowserClose;
    const next = on ? [...cur.filter((g) => g !== group), group] : cur.filter((g) => g !== group);
    setSmart({ cleanOnBrowserClose: next });
  };

  return (
    <Card title={t('settings.smart')} testId="settings-smart">
      <div className="flex flex-col gap-3">
        <SwitchRow
          label="Smart cleaning"
          hint="Watch for junk in the background and tell you, or clean it, when there is too much."
          checked={smart.enabled}
          onChange={(enabled) => setSmart({ enabled })}
          testId="smart-enabled"
        />

        <div className="text-sm">
          <span className="mb-1 block font-medium">Junk threshold</span>
          <div role="group" aria-label="Junk threshold presets" className="flex flex-wrap gap-1.5">
            {SMART_THRESHOLD_PRESETS.map((mb) => {
              const on = smart.thresholdMb === mb;
              return (
                <button
                  key={mb}
                  type="button"
                  aria-pressed={on}
                  onClick={() => pickPreset(mb)}
                  data-testid={`smart-threshold-${mb}`}
                  className={`h-10 rounded-full border px-3 text-xs font-medium ${
                    on ? 'border-accent bg-accent/10 text-accent-strong' : 'border-line bg-surface-2'
                  }`}
                >
                  {mb >= 1000 ? `${mb / 1000} GB` : `${mb} MB`}
                </button>
              );
            })}
          </div>
          <label className="mt-2 block">
            <span className="mb-1 block text-xs text-muted">Custom (MB)</span>
            <input
              type="number"
              inputMode="numeric"
              min={1}
              value={custom}
              onChange={(e) => setCustom(e.target.value)}
              onBlur={commitCustom}
              onKeyDown={(e) => {
                if (e.key === 'Enter') commitCustom();
              }}
              data-testid="smart-threshold-custom"
              className={selectClass}
            />
          </label>
          {customError && (
            <span className="mt-1 block text-xs text-danger" role="alert" data-testid="smart-threshold-error">
              {customError}
            </span>
          )}
        </div>

        <SwitchRow
          label="Notify me"
          hint="Show a notification when the junk passes the threshold (at most twice a day)."
          checked={smart.notify}
          onChange={(notify) => setSmart({ notify })}
          testId="smart-notify"
        />
        <SwitchRow
          label="Clean automatically"
          hint="Clean the junk right away instead of only telling you."
          checked={smart.autoClean}
          onChange={(autoClean) => setSmart({ autoClean })}
          testId="smart-auto-clean"
        />

        <div className="text-sm">
          <span className="mb-1 block font-medium">Clean when these browsers close</span>
          {browsers.length === 0 ? (
            <span className="block text-xs text-muted">No browsers were found on this computer.</span>
          ) : (
            <ul className="m-0 flex list-none flex-col gap-1 p-0">
              {browsers.map((b) => (
                <li key={b}>
                  <label className="flex min-h-10 min-w-0 items-center gap-2">
                    <Checkbox
                      checked={smart.cleanOnBrowserClose.includes(b)}
                      onChange={(on) => toggleBrowser(b, on)}
                      testId={`smart-browser-${slug(b)}`}
                      ariaLabel={`Clean when ${b} closes`}
                    />
                    <span className="min-w-0 break-words">{b}</span>
                  </label>
                </li>
              ))}
            </ul>
          )}
        </div>

        <label className="block text-sm">
          <span className="mb-1 block font-medium">Check for junk</span>
          <select
            value={smart.checkIntervalMinutes ?? 60}
            onChange={(e) => setSmart({ checkIntervalMinutes: Number(e.target.value) })}
            data-testid="smart-interval"
            className={selectClass}
          >
            {intervals.map((m) => (
              <option key={m} value={m}>
                {intervalLabel(m)}
              </option>
            ))}
          </select>
        </label>

        <p className="m-0 break-words text-xs text-muted" data-testid="smart-agent-status">
          {agentLine(status, formatWhen)}
        </p>
      </div>
    </Card>
  );
}
