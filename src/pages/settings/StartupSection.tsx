import { Card } from '../../components/Card';
import { t } from '../../i18n';
import { isTauri } from '../../lib/transport';
import { SwitchRow } from './SwitchRow';
import type { SectionProps } from './types';

export function StartupSection({ settings, patch }: SectionProps) {
  return (
    <Card title={t('settings.startup')} testId="settings-startup">
      <div className="flex flex-col gap-3">
        <SwitchRow
          label="Run at startup"
          hint="Start the ClearSweep background agent when you log in, so smart cleaning and schedules keep working without the window."
          checked={settings.runAtStartup}
          onChange={(runAtStartup) => void patch({ runAtStartup })}
          testId="setting-run-at-startup"
        />
        {isTauri() && (
          <SwitchRow
            label="Close to tray"
            hint="The close button hides the window and keeps ClearSweep running in the tray. Use Quit in the tray menu to exit."
            checked={settings.closeToTray ?? true}
            onChange={(closeToTray) => void patch({ closeToTray })}
            testId="setting-close-to-tray"
          />
        )}
      </div>
    </Card>
  );
}
