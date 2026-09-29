import type { Plugin, PluginType } from '../api/browser_plugins';

export interface ProfileGroup {
  profile: string;
  profileName?: string;
  plugins: Plugin[];
}

export interface BrowserGroup {
  browser: string;
  browserLabel: string;
  running: boolean;
  profiles: ProfileGroup[];
}

export const TYPE_LABEL: Record<PluginType, string> = {
  extension: 'Extension',
  theme: 'Theme',
  app: 'App',
  plugin: 'Plugin',
  dictionary: 'Dictionary',
  locale: 'Language',
};

/** Browser -> profile -> plugins, preserving the server's order. */
export function groupPlugins(plugins: Plugin[]): BrowserGroup[] {
  const out: BrowserGroup[] = [];
  for (const p of plugins) {
    let b = out.find((x) => x.browser === p.browser);
    if (!b) {
      b = { browser: p.browser, browserLabel: p.browserLabel, running: false, profiles: [] };
      out.push(b);
    }
    if (p.running) b.running = true;
    let pr = b.profiles.find((x) => x.profile === p.profile);
    if (!pr) {
      pr = { profile: p.profile, profileName: p.profileName, plugins: [] };
      b.profiles.push(pr);
    }
    pr.plugins.push(p);
  }
  return out;
}

export function filterPlugins(plugins: Plugin[], query: string): Plugin[] {
  const q = query.trim().toLowerCase();
  if (!q) return plugins;
  return plugins.filter(
    (p) => p.name.toLowerCase().includes(q) || p.description.toLowerCase().includes(q) || p.extensionId.toLowerCase().includes(q),
  );
}

/** "Close Google Chrome first: ..." -> "Google Chrome"; null for any other error. */
export function closeBrowserName(message: string): string | null {
  const m = /^Close (.+?) first\b/.exec(message);
  return m ? (m[1] ?? null) : null;
}

/** Title of the profile heading. */
export function profileTitle(p: ProfileGroup): string {
  return p.profileName && p.profileName !== p.profile ? `${p.profileName} (${p.profile})` : p.profile;
}
