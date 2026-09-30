import { FolderOpen } from 'lucide-react';
import { useEffect, useState } from 'react';
import pkg from '../../../package.json';
import type { AppInfo } from '../../api/system';
import { Card } from '../../components/Card';
import { t } from '../../i18n';
import { ApiCallError, call } from '../../lib/transport';

export function AboutSection() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [openError, setOpenError] = useState<{ message: string; unsupported: boolean } | null>(null);

  useEffect(() => {
    let live = true;
    call<AppInfo>('system.app_info')
      .then((i) => live && setInfo(i))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, []);

  const openData = async () => {
    setOpenError(null);
    try {
      await call('system.open_data_dir');
    } catch (e) {
      const err = ApiCallError.from(e);
      setOpenError({ message: err.message, unsupported: err.code === 'Unsupported' });
    }
  };

  return (
    <Card title={t('settings.about')} testId="settings-about">
      <div className="flex flex-col gap-2 text-sm">
        <p className="m-0">
          <span className="font-semibold" data-testid="about-name">
            {info?.name ?? 'ClearSweep'}
          </span>{' '}
          <span className="text-muted" data-testid="about-version">
            version {info?.version ?? pkg.version}
          </span>
        </p>
        <p className="m-0 break-words" data-testid="about-promise">
          All features are free. No account, no ads, no telemetry.
        </p>
        <p className="m-0 text-xs text-muted" data-testid="about-license">
          License: {info?.license ?? 'MIT'}
        </p>
        {info?.dataDir && (
          <p className="m-0 break-all text-xs text-muted" data-testid="about-data-dir">
            Data folder: {info.dataDir}
          </p>
        )}
        <button
          type="button"
          onClick={() => void openData()}
          data-testid="about-open-data"
          className="flex h-10 w-full items-center justify-center gap-2 rounded-xl border border-line bg-surface-2 text-sm font-medium"
        >
          <FolderOpen size={16} aria-hidden /> Open data folder
        </button>
        {openError && (
          <span
            className={`text-xs ${openError.unsupported ? 'text-muted' : 'text-danger'}`}
            role={openError.unsupported ? 'status' : 'alert'}
            data-testid="about-open-error"
          >
            {openError.message}
          </span>
        )}
      </div>
    </Card>
  );
}
