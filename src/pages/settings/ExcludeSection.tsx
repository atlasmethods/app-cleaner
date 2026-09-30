import { useState } from 'react';
import { ShieldPlus, X } from 'lucide-react';
import { Card } from '../../components/Card';
import { t } from '../../i18n';
import type { SectionProps } from './types';

export function ExcludeSection({ settings, patch }: SectionProps) {
  const [pattern, setPattern] = useState('');

  const add = async () => {
    const p = pattern.trim();
    if (!p) return;
    const next = await patch({
      exclude: [...settings.exclude, { id: globalThis.crypto.randomUUID(), pattern: p }],
    });
    if (next) setPattern('');
  };

  const remove = (id: string) => void patch({ exclude: settings.exclude.filter((e) => e.id !== id) });

  return (
    <Card title={t('settings.exclude')} testId="settings-exclude">
      <p className="m-0 mb-2 text-xs text-muted">
        Files and folders that are never deleted, by any cleaning. Use an absolute path or a pattern such as
        ~/Downloads/keep*. A folder protects everything inside it.
      </p>
      {settings.exclude.length > 0 && (
        <ul className="m-0 mb-3 list-none divide-y divide-line p-0" data-testid="exclude-list">
          {settings.exclude.map((e) => (
            <li key={e.id} data-testid={`exclude-${e.id}`} className="flex min-w-0 items-center gap-2 py-1.5">
              <span className="min-w-0 flex-1 break-all text-sm">{e.pattern}</span>
              <button
                type="button"
                onClick={() => remove(e.id)}
                aria-label={`Remove ${e.pattern}`}
                data-testid={`exclude-remove-${e.id}`}
                className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full border-0 bg-transparent"
              >
                <X size={16} aria-hidden />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex flex-col gap-2">
        <input
          value={pattern}
          onChange={(e) => setPattern(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void add();
          }}
          placeholder="Path or pattern to protect"
          aria-label="Path or pattern to exclude"
          data-testid="exclude-pattern"
          className="h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 px-2 text-sm"
        />
        <button
          type="button"
          onClick={() => void add()}
          disabled={!pattern.trim()}
          data-testid="exclude-add"
          className="flex h-10 items-center justify-center gap-1.5 rounded-xl border border-line bg-surface-2 text-sm font-medium disabled:opacity-50"
        >
          <ShieldPlus size={16} aria-hidden /> Add exclusion
        </button>
      </div>
    </Card>
  );
}
