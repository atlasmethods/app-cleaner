import { Monitor, Moon, Sun, type LucideIcon } from 'lucide-react';
import type { Theme } from '../../api/settings';
import { Card } from '../../components/Card';
import { applyTheme } from '../../lib/theme';
import type { SectionProps } from './types';

const THEMES: { id: Theme; label: string; icon: LucideIcon }[] = [
  { id: 'system', label: 'System', icon: Monitor },
  { id: 'light', label: 'Light', icon: Sun },
  { id: 'dark', label: 'Dark', icon: Moon },
];

export function AppearanceSection({ settings, patch }: SectionProps) {
  const choose = (theme: Theme) => {
    applyTheme(theme); // immediate; the server copy makes it survive restarts
    void patch({ theme });
  };
  return (
    <Card title="Appearance" testId="settings-appearance">
      <div role="radiogroup" aria-label="Theme" className="grid grid-cols-3 gap-2">
        {THEMES.map(({ id, label, icon: Icon }) => {
          const on = settings.theme === id;
          return (
            <button
              key={id}
              type="button"
              role="radio"
              aria-checked={on}
              onClick={() => choose(id)}
              data-testid={`theme-${id}`}
              className={`flex h-14 min-w-0 flex-col items-center justify-center gap-0.5 rounded-xl border text-xs font-medium ${
                on ? 'border-accent bg-accent/10 text-accent-strong' : 'border-line bg-surface-2'
              }`}
            >
              <Icon size={18} aria-hidden />
              {label}
            </button>
          );
        })}
      </div>
    </Card>
  );
}
