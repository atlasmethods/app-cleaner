import type { UpdateEntry, UpdateResult, UpdateSource } from '../api/software_updater';

const SOURCE_LABEL: Record<UpdateSource, string> = {
  apt: 'apt',
  dnf: 'dnf',
  pacman: 'pacman',
  flatpak: 'Flatpak',
  snap: 'Snap',
  winget: 'winget',
  brew: 'Homebrew',
  'brew-cask': 'Cask',
  macos: 'macOS',
};

export function updateSourceLabel(s: UpdateSource): string {
  return SOURCE_LABEL[s];
}

/** Entries the user can select (ignored ones cannot be). */
export function selectable(entries: readonly UpdateEntry[]): UpdateEntry[] {
  return entries.filter((e) => !e.ignored);
}

/** Selection restricted to ids that still exist and are not ignored. */
export function effectiveSelection(entries: readonly UpdateEntry[], chosen: ReadonlySet<string>): string[] {
  return selectable(entries)
    .map((e) => e.id)
    .filter((id) => chosen.has(id));
}

export function toggleId(set: ReadonlySet<string>, id: string): Set<string> {
  const next = new Set(set);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

/** "126.0 -> 127.0" (the current version may be unknown). */
export function versionChange(e: Pick<UpdateEntry, 'currentVersion' | 'newVersion'>): string {
  const cur = e.currentVersion.trim();
  const next = e.newVersion.trim() || 'newer';
  return cur ? `${cur} → ${next}` : `→ ${next}`;
}

export function resultById(results: readonly UpdateResult[]): Map<string, UpdateResult> {
  return new Map(results.map((r) => [r.id, r]));
}

/** Update ids to run for "Update all": everything not ignored. */
export function allUpdatable(entries: readonly UpdateEntry[]): number {
  return selectable(entries).length;
}
