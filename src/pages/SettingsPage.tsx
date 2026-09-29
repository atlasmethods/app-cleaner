import type { ComponentType } from 'react';
import { ErrorBanner } from '../components/ErrorBanner';
import { useSettings } from '../hooks/useSettings';
import { AppearanceSection } from './settings/AppearanceSection';
import { CleaningSection } from './settings/CleaningSection';
import { ExcludeSection } from './settings/ExcludeSection';
import { IncludeSection } from './settings/IncludeSection';
import type { SectionProps } from './settings/types';

/** Add a settings section: create a component in ./settings and list it here. */
const SECTIONS: { id: string; Component: ComponentType<SectionProps> }[] = [
  { id: 'appearance', Component: AppearanceSection },
  { id: 'cleaning', Component: CleaningSection },
  { id: 'include', Component: IncludeSection },
  { id: 'exclude', Component: ExcludeSection },
];

export default function SettingsPage() {
  const { settings, error, loading, patch, reload, clearError } = useSettings();
  return (
    <div data-testid="page-settings" className="flex flex-col gap-3 p-4">
      <ErrorBanner error={error} onRetry={settings ? clearError : () => void reload()} />
      {!settings && loading && <p className="py-8 text-center text-sm text-muted">Loading settings...</p>}
      {settings &&
        SECTIONS.map(({ id, Component }) => (
          <Component key={id} settings={settings} patch={patch} />
        ))}
    </div>
  );
}
