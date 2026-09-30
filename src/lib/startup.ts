import type { StartupImpact, StartupItem, StartupKind } from '../api/startup';

/** Chip label per kind, in display order. */
export const KIND_ORDER: StartupKind[] = [
  'autostart',
  'login_item',
  'launch_agent',
  'launch_daemon',
  'service',
  'scheduled_task',
  'cron',
  'context_menu',
];

export const KIND_LABEL: Record<StartupKind, string> = {
  autostart: 'Autostart',
  login_item: 'Login items',
  launch_agent: 'Launch agents',
  launch_daemon: 'Launch daemons',
  service: 'Services',
  scheduled_task: 'Scheduled tasks',
  cron: 'Cron',
  context_menu: 'Context menu',
};

export const IMPACT_LABEL: Record<StartupImpact, string> = {
  high: 'High',
  medium: 'Medium',
  low: 'Low',
  unknown: 'Unknown',
};

export interface Filter {
  query: string;
  kind: StartupKind | 'all';
  showSystem: boolean;
}

/** Items the user may see with the current "Show system items" setting. */
function shownBySystemSetting(items: StartupItem[], showSystem: boolean): StartupItem[] {
  return showSystem ? items : items.filter((i) => !i.critical);
}

/** Kinds that have at least one visible item, with counts, in a fixed order. */
export function kindsPresent(items: StartupItem[], showSystem: boolean): { kind: StartupKind; label: string; count: number }[] {
  const pool = shownBySystemSetting(items, showSystem);
  return KIND_ORDER.map((kind) => ({
    kind,
    label: KIND_LABEL[kind],
    count: pool.filter((i) => i.kind === kind).length,
  })).filter((k) => k.count > 0);
}

export function hiddenSystemCount(items: StartupItem[]): number {
  return items.filter((i) => i.critical).length;
}

export function visibleItems(items: StartupItem[], f: Filter): StartupItem[] {
  const q = f.query.trim().toLowerCase();
  return shownBySystemSetting(items, f.showSystem)
    .filter((i) => f.kind === 'all' || i.kind === f.kind)
    .filter(
      (i) =>
        q === '' ||
        i.name.toLowerCase().includes(q) ||
        (i.publisher ?? '').toLowerCase().includes(q) ||
        i.command.toLowerCase().includes(q),
    );
}

/** Text of the delete confirmation: says what happens and that a backup is kept. */
export function deleteMessage(item: StartupItem): string {
  const what: Record<StartupKind, string> = {
    autostart: 'The startup entry is removed. The program itself is not uninstalled.',
    login_item: 'The login item is removed. The program itself is not uninstalled.',
    launch_agent: 'The launch agent file is removed. The program itself is not uninstalled.',
    launch_daemon: 'The launch daemon is removed.',
    service: 'The service is removed.',
    scheduled_task: 'The scheduled task is deleted.',
    cron: 'The @reboot line is removed from your crontab; every other line stays as it is.',
    context_menu: 'The context menu entry is removed.',
  };
  return `${what[item.kind]} A backup is saved first, so you can restore it from System Restore.`;
}

/** Sub-line under the name: publisher, else the command. */
export function subline(item: StartupItem): string {
  const parts = [item.publisher, item.scope === 'system' ? 'All users' : null].filter(Boolean);
  return parts.length > 0 ? parts.join(' - ') : item.command;
}
