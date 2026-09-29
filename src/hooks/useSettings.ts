import { useCallback, useEffect, useState } from 'react';
import type { Settings, SettingsPatch } from '../api/settings';
import { ApiCallError, call } from '../lib/transport';
import { useCall } from './useCall';

export interface UseSettings {
  settings: Settings | null;
  error: ApiCallError | null;
  loading: boolean;
  /** Merge-patch the settings on the server; resolves with the new settings, or undefined on error. */
  patch: (p: SettingsPatch) => Promise<Settings | undefined>;
  reload: () => Promise<void>;
  clearError: () => void;
}

/** Loads `settings.get` and keeps the returned copy in sync with every `settings.set`. */
export function useSettings(): UseSettings {
  const get = useCall<Settings>('settings.get');
  // The newest copy returned by `settings.set`; supersedes what `settings.get` returned.
  const [saved, setSaved] = useState<Settings | null>(null);
  const [patchError, setPatchError] = useState<ApiCallError | null>(null);

  const load = get.run;
  useEffect(() => {
    void load();
  }, [load]);

  const patch = useCallback(async (p: SettingsPatch) => {
    try {
      const next = await call<Settings>('settings.set', p);
      setSaved(next);
      setPatchError(null);
      return next;
    } catch (e) {
      setPatchError(ApiCallError.from(e));
      return undefined;
    }
  }, []);

  const reload = useCallback(async () => {
    setSaved(null);
    setPatchError(null);
    await load();
  }, [load]);

  const clearError = useCallback(() => setPatchError(null), []);

  return {
    settings: saved ?? get.data,
    error: patchError ?? get.error,
    loading: get.loading,
    patch,
    reload,
    clearError,
  };
}
