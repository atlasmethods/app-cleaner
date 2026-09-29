import {
  Cookie,
  Database,
  Eraser,
  Gauge,
  History,
  House,
  Info,
  Copy,
  PieChart,
  Power,
  Puzzle,
  RefreshCw,
  Settings,
  Sparkles,
  Trash2,
  Wrench,
  HardDrive,
  type LucideIcon,
} from 'lucide-react';

export interface TabDef {
  id: 'home' | 'clean' | 'tools' | 'performance' | 'settings';
  label: string;
  path: string;
  icon: LucideIcon;
}

export const TABS: TabDef[] = [
  { id: 'home', label: 'Home', path: '/', icon: House },
  { id: 'clean', label: 'Clean', path: '/clean', icon: Sparkles },
  { id: 'tools', label: 'Tools', path: '/tools', icon: Wrench },
  { id: 'performance', label: 'Performance', path: '/performance', icon: Gauge },
  { id: 'settings', label: 'Settings', path: '/settings', icon: Settings },
];

export interface ToolDef {
  /** Used for `tile-<id>` and `page-<id>` test ids and the route `/tools/<id>`. */
  id: string;
  label: string;
  path: string;
  icon: LucideIcon;
}

const tool = (id: string, label: string, icon: LucideIcon): ToolDef => ({
  id,
  label,
  icon,
  path: `/tools/${id}`,
});

export const TOOLS: ToolDef[] = [
  tool('uninstall', 'Uninstall', Trash2),
  tool('updater', 'Software Updater', RefreshCw),
  tool('drivers', 'Driver Updater', HardDrive),
  tool('startup', 'Startup', Power),
  tool('plugins', 'Browser Plugins', Puzzle),
  tool('disk', 'Disk Analyzer', PieChart),
  tool('duplicates', 'Duplicate Finder', Copy),
  tool('restore', 'System Restore', History),
  tool('wiper', 'Drive Wiper', Eraser),
  tool('registry', 'Registry / Config Issues', Database),
  tool('sysinfo', 'System Info', Info),
  tool('cookies', 'Cookies', Cookie),
];

/** Title + optional parent (drill-in pages get a back button) for a pathname. */
export function routeMeta(pathname: string): { title: string; parent: string | null } {
  const t = TOOLS.find((x) => x.path === pathname);
  if (t) return { title: t.label, parent: '/tools' };
  const tab = TABS.find((x) => x.path === pathname);
  return { title: tab?.label ?? 'ClearSweep', parent: null };
}
