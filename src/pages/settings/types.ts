import type { Settings, SettingsPatch } from '../../api/settings';

/** Props every settings section receives. Add a section = new component + one line in SettingsPage. */
export interface SectionProps {
  settings: Settings;
  /** Merge-patch the persisted settings; resolves with the new settings or undefined on error. */
  patch: (p: SettingsPatch) => Promise<Settings | undefined>;
}
