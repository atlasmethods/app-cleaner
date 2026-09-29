import type { AppEntry, AppSource } from '../api/uninstall';

export type SortKey = 'name' | 'size' | 'date';

export interface ListOptions {
  query: string;
  showSystem: boolean;
  sort: SortKey;
}

const byName = (a: AppEntry, b: AppEntry) =>
  a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }) || a.id.localeCompare(b.id);

/** Missing values sort last; ties fall back to the name. */
function cmpDesc<T extends number | string>(a: T | undefined, b: T | undefined): number {
  if (a === undefined && b === undefined) return 0;
  if (a === undefined) return 1;
  if (b === undefined) return -1;
  return a < b ? 1 : a > b ? -1 : 0;
}

/** Filter (search text, system components) and sort the application list. */
export function visibleApps(apps: readonly AppEntry[], opts: ListOptions): AppEntry[] {
  const q = opts.query.trim().toLowerCase();
  const list = apps.filter(
    (a) =>
      (opts.showSystem || !a.isSystem) &&
      (q === '' || a.name.toLowerCase().includes(q) || a.publisher.toLowerCase().includes(q)),
  );
  const sorted = [...list];
  switch (opts.sort) {
    case 'size':
      sorted.sort((a, b) => cmpDesc(a.sizeBytes, b.sizeBytes) || byName(a, b));
      break;
    case 'date':
      sorted.sort((a, b) => cmpDesc(a.installDate, b.installDate) || byName(a, b));
      break;
    default:
      sorted.sort(byName);
  }
  return sorted;
}

export interface AppActions {
  uninstall: boolean;
  repair: boolean;
  renameEntry: boolean;
  removeEntry: boolean;
}

/** Only the actions the backend supports for this entry. */
export function actionsFor(app: AppEntry): AppActions {
  const windows = app.source === 'windows';
  return {
    uninstall: app.uninstallable,
    repair: app.canRepair && windows,
    renameEntry: windows,
    removeEntry: windows,
  };
}

const SOURCE_LABEL: Record<AppSource, string> = {
  dpkg: 'deb',
  rpm: 'rpm',
  pacman: 'pacman',
  flatpak: 'Flatpak',
  snap: 'Snap',
  appimage: 'AppImage',
  windows: 'Windows',
  macapp: 'App',
  brew: 'Homebrew',
};

export function sourceLabel(s: AppSource): string {
  return SOURCE_LABEL[s];
}

/** "1.2.3 - Publisher" (either part may be missing). */
export function subtitle(app: AppEntry): string {
  return [app.version, app.publisher].filter((s) => s.trim() !== '').join(' - ');
}

/** Warning line for the uninstall confirmation. */
export function uninstallWarning(app: AppEntry): string {
  if (app.isSystem) {
    return `${app.name} is a system component. Removing it can break your system or other programs.`;
  }
  if (app.source === 'appimage') return `This deletes the AppImage file ${app.name}.`;
  if (app.source === 'macapp') return `${app.name} will be moved to the Trash.`;
  return `${app.name} will be removed from this computer.`;
}
