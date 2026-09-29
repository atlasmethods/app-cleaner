import { useState } from 'react';
import type { CloseBrowsers, Passes } from '../../api/settings';
import { Card } from '../../components/Card';
import { Checkbox } from '../../components/Checkbox';
import type { SectionProps } from './types';

const PASSES: { value: Passes; label: string }[] = [
  { value: 1, label: '1 pass (fast)' },
  { value: 3, label: '3 passes (DoD 5220.22-M)' },
  { value: 7, label: '7 passes' },
  { value: 35, label: '35 passes (Gutmann, very slow)' },
];

const CLOSE: { value: CloseBrowsers; label: string }[] = [
  { value: 'ask', label: 'Ask me' },
  { value: 'always', label: 'Close them automatically' },
  { value: 'skip', label: 'Skip them' },
];

const selectClass = 'h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 px-2 text-sm';

export function CleaningSection({ settings, patch }: SectionProps) {
  const [age, setAge] = useState(String(settings.tempMinAgeHours));
  const [ageError, setAgeError] = useState<string | null>(null);

  const commitAge = () => {
    const n = Number(age);
    if (!Number.isInteger(n) || n < 0 || n > 87600) {
      setAgeError('Enter a whole number of hours between 0 and 87600.');
      return;
    }
    setAgeError(null);
    if (n !== settings.tempMinAgeHours) void patch({ tempMinAgeHours: n });
  };

  return (
    <Card title="Cleaning" testId="settings-cleaning">
      <div className="flex flex-col gap-3">
        <label className="flex min-w-0 items-start gap-2 text-sm">
          <span className="mt-0.5">
            <Checkbox
              checked={settings.secureDelete.enabled}
              onChange={(enabled) => void patch({ secureDelete: { enabled } })}
              testId="setting-secure-enabled"
              ariaLabel="Secure deletion"
            />
          </span>
          <span className="min-w-0">
            <span className="block font-medium">Secure deletion</span>
            <span className="block text-xs text-muted">
              Overwrite files before deleting them. Slower, and not guaranteed on SSDs or snapshotting file systems.
            </span>
          </span>
        </label>

        <label className="block text-sm">
          <span className="mb-1 block font-medium">Overwrite passes</span>
          <select
            value={settings.secureDelete.passes}
            disabled={!settings.secureDelete.enabled}
            onChange={(e) => void patch({ secureDelete: { passes: Number(e.target.value) as Passes } })}
            data-testid="setting-secure-passes"
            className={`${selectClass} disabled:opacity-50`}
          >
            {PASSES.map((p) => (
              <option key={p.value} value={p.value}>
                {p.label}
              </option>
            ))}
          </select>
        </label>

        <label className="block text-sm">
          <span className="mb-1 block font-medium">When a browser is running</span>
          <select
            value={settings.closeBrowsers}
            onChange={(e) => void patch({ closeBrowsers: e.target.value as CloseBrowsers })}
            data-testid="setting-close-browsers"
            className={selectClass}
          >
            {CLOSE.map((c) => (
              <option key={c.value} value={c.value}>
                {c.label}
              </option>
            ))}
          </select>
        </label>

        <label className="block text-sm">
          <span className="mb-1 block font-medium">Keep temp files newer than (hours)</span>
          <input
            type="number"
            inputMode="numeric"
            min={0}
            value={age}
            onChange={(e) => setAge(e.target.value)}
            onBlur={commitAge}
            onKeyDown={(e) => {
              if (e.key === 'Enter') commitAge();
            }}
            data-testid="setting-temp-age"
            className={selectClass}
          />
          {ageError && (
            <span className="mt-1 block text-xs text-danger" role="alert">
              {ageError}
            </span>
          )}
        </label>
      </div>
    </Card>
  );
}
