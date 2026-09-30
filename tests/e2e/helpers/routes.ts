/** Every route of the app (mirrors src/nav.ts; app.spec.ts checks the tiles against the real UI). */
export const TABS = ['home', 'clean', 'tools', 'performance', 'settings'] as const;

export const TOOL_TILES: { id: string; title: string }[] = [
  { id: 'uninstall', title: 'Uninstall' },
  { id: 'updater', title: 'Software Updater' },
  { id: 'drivers', title: 'Driver Updater' },
  { id: 'startup', title: 'Startup' },
  { id: 'plugins', title: 'Browser Plugins' },
  { id: 'disk', title: 'Disk Analyzer' },
  { id: 'duplicates', title: 'Duplicate Finder' },
  { id: 'restore', title: 'System Restore' },
  { id: 'wiper', title: 'Drive Wiper' },
  { id: 'shredder', title: 'File Shredder' },
  { id: 'registry', title: 'Config Issues' },
  { id: 'sysinfo', title: 'System Info' },
  { id: 'cookies', title: 'Cookies' },
];

export const ALL_ROUTES = [
  '/',
  '/clean',
  '/clean/history',
  '/tools',
  ...TOOL_TILES.map((t) => `/tools/${t.id}`),
  '/performance',
  '/settings',
  '/settings/schedules',
];
