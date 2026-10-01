/** Mirrors `sweep_core::features::startup` (serde camelCase / snake_case enums). */
export const STARTUP_METHODS = [
  'startup.list',
  'startup.set_enabled',
  'startup.remove',
  'startup.restore_backup',
] as const;

export type StartupMethod = (typeof STARTUP_METHODS)[number];

export type StartupKind =
  | 'autostart'
  | 'service'
  | 'scheduled_task'
  | 'context_menu'
  | 'cron'
  | 'launch_agent'
  | 'launch_daemon'
  | 'login_item';

export type StartupScope = 'user' | 'system';
export type StartupImpact = 'high' | 'medium' | 'low' | 'unknown';

export interface StartupItem {
  id: string;
  name: string;
  command: string;
  location: string;
  kind: StartupKind;
  scope: StartupScope;
  enabled: boolean;
  publisher?: string;
  impact: StartupImpact;
  canDisable: boolean;
  canDelete: boolean;
  /** Needed by the system: shown locked, never changed. */
  critical: boolean;
  warning?: string;
}

export interface SetEnabledResult {
  ok: boolean;
  item: StartupItem;
}

export interface RemoveResult {
  ok: boolean;
  id: string;
  backupId: string;
  backupPath: string;
}

export interface RestoreStartupResult {
  ok: boolean;
  id: string;
  restored: number;
  /** Things left as they were (for example a file that already exists again). */
  notes: string[];
  name?: string | null;
}
