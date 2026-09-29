import type { PointKind, RestorePoint } from '../api/restore';

export const KIND_LABEL: Record<PointKind, string> = {
  'windows-restore-point': 'Windows',
  timeshift: 'Timeshift',
  snapper: 'Snapper',
  tmutil: 'Time Machine',
  'clearsweep-backup': 'ClearSweep backup',
};

export function kindLabel(kind: PointKind): string {
  return KIND_LABEL[kind] ?? kind;
}

/** Why a point cannot be deleted (shown next to the disabled button), or null when it can. */
export function deleteBlockedReason(p: RestorePoint): string | null {
  if (p.deletable) return null;
  if (p.isNewest) return 'The most recent restore point cannot be deleted';
  if (p.kind === 'windows-restore-point') {
    return 'Windows can only delete restore points in bulk (use "Delete all but the most recent")';
  }
  return 'This restore point cannot be deleted';
}

export function backupKindLabel(p: RestorePoint): string {
  switch (p.backupKind) {
    case 'registry':
      return 'Registry backup';
    case 'config':
      return 'Configuration backup';
    case 'uninstall-entry':
      return 'Uninstall entry backup';
    case 'drivers':
      return 'Driver backup';
    default:
      return 'Backup';
  }
}

/** Points the user can roll back to with ClearSweep itself. */
export function restorablePoints(points: RestorePoint[]): RestorePoint[] {
  return points.filter((p) => p.restorable);
}

/** More than one Windows restore point exists, so "delete all but the newest" does something. */
export function hasOlderWindowsPoints(points: RestorePoint[]): boolean {
  return points.filter((p) => p.kind === 'windows-restore-point').length > 1;
}

/** Split into the system's own points and ClearSweep's backups, keeping the order. */
export function splitPoints(points: RestorePoint[]): { system: RestorePoint[]; backups: RestorePoint[] } {
  return {
    system: points.filter((p) => p.kind !== 'clearsweep-backup'),
    backups: points.filter((p) => p.kind === 'clearsweep-backup'),
  };
}

/** Validates the description typed by the user the same way the server does. */
export function descriptionProblem(d: string): string | null {
  const t = d.trim();
  if (t.length === 0) return 'Enter a description';
  if (t.length > 200) return 'Use at most 200 characters';
  if (t.startsWith('-')) return 'The description cannot start with a dash';
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(t)) return 'The description cannot contain control characters';
  return null;
}
