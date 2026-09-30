import { Card } from '../../components/Card';
import { LANGUAGES, setLanguage, t } from '../../i18n';
import type { SectionProps } from './types';

export function LanguageSection({ settings, patch }: SectionProps) {
  const known = LANGUAGES.some((l) => l.code === settings.language);
  return (
    <Card title={t('settings.language')} testId="settings-language">
      <select
        aria-label="Language"
        value={known ? settings.language : 'en'}
        onChange={(e) => {
          setLanguage(e.target.value);
          void patch({ language: e.target.value });
        }}
        data-testid="setting-language"
        className="h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 px-2 text-sm"
      >
        {LANGUAGES.map((l) => (
          <option key={l.code} value={l.code}>
            {l.label}
          </option>
        ))}
      </select>
      <p className="mb-0 mt-2 text-xs text-muted">More languages will follow.</p>
    </Card>
  );
}
