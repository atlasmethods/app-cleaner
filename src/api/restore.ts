/** Mirrors `sweep_core::features::restore` (serde camelCase). */
export const RESTORE_METHODS = [
  'restore.list_points',
  'restore.create_point',
  'restore.delete_point',
  'restore.delete_old',
  'restore.restore',
  'restore.open_system_tool',
] as const;

export type RestoreMethod = (typeof RESTORE_METHODS)[number];

export type PointKind = 'windows-restore-point' | 'timeshift' | 'snapper' | 'tmutil' | 'clearsweep-backup';
export type BackupKind = 'registry' | 'config' | 'uninstall-entry' | 'drivers' | 'startup' | 'plugins';

export interface RestorePoint {
  id: string;
  description: string;
  /** ISO 8601, sometimes without an offset (local time). */
  createdAt: string | null;
  kind: PointKind;
  sizeBytes?: number;
  deletable: boolean;
  /** ClearSweep can put it back itself. */
  restorable: boolean;
  /** The newest point of its tool: never deletable. */
  isNewest: boolean;
  note?: string;
  backupKind?: BackupKind;
}

export interface ListPointsResult {
  os: 'windows' | 'linux' | 'macos';
  supported: boolean;
  tool: string | null;
  tools: string[];
  hint: string | null;
  canCreate: boolean;
  canDeleteOld: boolean;
  canOpenSystemTool: boolean;
  /** Listing the system's points needs administrator rights (call again with `elevate`). */
  needsAdmin: boolean;
  warnings: string[];
  points: RestorePoint[];
}

export interface OpResult {
  ok: boolean;
  message: string;
  throttled?: boolean;
  deleted?: number;
  remaining?: number;
}
