import type {
  AppFinding,
  CategoryId,
  FixParams,
  FixPart,
  FixReport,
  HealthCategory,
  HealthReport,
  HealthStatus,
  PartResult,
  StartupFinding,
  UpdateFinding,
} from '../api/health';
import { formatBytes } from './format';

// ---------------------------------------------------------------- reading a report

export const CATEGORY_ORDER: CategoryId[] = ['privacy', 'space', 'speed', 'security'];

export function categoryOf(report: HealthReport, id: CategoryId): HealthCategory | undefined {
  return report.categories.find((c) => c.id === id);
}

export function startupItems(report: HealthReport): StartupFinding[] {
  const out: StartupFinding[] = [];
  for (const f of categoryOf(report, 'speed')?.findings ?? []) if (f.kind === 'startup') out.push(...f.items);
  return out;
}

export function backgroundApps(report: HealthReport): AppFinding[] {
  const out: AppFinding[] = [];
  for (const f of categoryOf(report, 'speed')?.findings ?? []) if (f.kind === 'background_apps') out.push(...f.apps);
  return out;
}

export function updateItems(report: HealthReport): UpdateFinding[] {
  const out: UpdateFinding[] = [];
  for (const f of categoryOf(report, 'security')?.findings ?? []) if (f.kind === 'updates') out.push(...f.items);
  return out;
}

/** Fixable and measured (an unavailable category never offers a fix). */
export function canFix(c: HealthCategory | undefined): boolean {
  return !!c && c.status !== 'unavailable' && c.fixable;
}

/** Browsers named by the privacy findings (trackers and history), each once. */
export function privacyBrowsers(c: HealthCategory | undefined): string[] {
  const set = new Set<string>();
  for (const f of c?.findings ?? []) {
    if (f.kind === 'trackers' || f.kind === 'history') for (const b of f.browsers) set.add(b);
  }
  return [...set];
}

export function trackerCount(c: HealthCategory | undefined): number {
  return (c?.findings ?? []).reduce((n, f) => (f.kind === 'trackers' ? n + f.count : n), 0);
}

export function historyCount(c: HealthCategory | undefined): number {
  return (c?.findings ?? []).reduce((n, f) => (f.kind === 'history' ? n + f.count : n), 0);
}

// ---------------------------------------------------------------- selection

export interface Selection {
  privacy: boolean;
  space: boolean;
  startup: ReadonlySet<string>;
  apps: ReadonlySet<string>;
  updates: ReadonlySet<string>;
}

/**
 * What is ticked before the user changes anything: every fixable finding, except updates that
 * are not security updates. Updating software can change how programs behave and is slow, so
 * those are the user's choice; security updates are pre-selected.
 */
export function defaultSelection(report: HealthReport): Selection {
  return {
    privacy: canFix(categoryOf(report, 'privacy')),
    space: canFix(categoryOf(report, 'space')),
    startup: new Set(canFix(categoryOf(report, 'speed')) ? startupItems(report).map((i) => i.id) : []),
    apps: new Set(canFix(categoryOf(report, 'speed')) ? backgroundApps(report).map((a) => a.appId) : []),
    updates: new Set(
      canFix(categoryOf(report, 'security')) ? updateItems(report).filter((u) => u.security).map((u) => u.id) : [],
    ),
  };
}

export function toggleIn(set: ReadonlySet<string>, id: string): Set<string> {
  const next = new Set(set);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

/**
 * The `health.fix` parameters for a selection: only what the report actually offers, so an
 * id that is not in the report (or a category that cannot be fixed) is never sent.
 */
export function toFixParams(report: HealthReport, sel: Selection): FixParams {
  const p: FixParams = {};
  if (sel.privacy && canFix(categoryOf(report, 'privacy'))) p.privacy = true;
  if (sel.space && canFix(categoryOf(report, 'space'))) p.space = true;
  const startup = startupItems(report)
    .map((i) => i.id)
    .filter((id) => sel.startup.has(id));
  if (startup.length > 0) p.startupIds = startup;
  const apps = backgroundApps(report)
    .map((a) => a.appId)
    .filter((id) => sel.apps.has(id));
  if (apps.length > 0) p.sleepAppIds = apps;
  const updates = updateItems(report)
    .map((u) => u.id)
    .filter((id) => sel.updates.has(id));
  if (updates.length > 0) p.updateIds = updates;
  return p;
}

export function isEmptyFix(p: FixParams): boolean {
  return !p.privacy && !p.space && !p.startupIds?.length && !p.sleepAppIds?.length && !p.updateIds?.length;
}

function count(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/**
 * One sentence fragment naming exactly what a fix will do, for the confirmation sheet:
 * "Delete 1.2 GB of junk, remove tracking cookies from 3 browsers, disable 4 startup items,
 * put 2 apps to sleep, install 3 updates".
 */
export function describeFix(report: HealthReport, p: FixParams): string {
  const parts: string[] = [];
  if (p.privacy) {
    const c = categoryOf(report, 'privacy');
    const what: string[] = [];
    if (trackerCount(c) > 0) what.push('remove tracking cookies');
    if (historyCount(c) > 0) what.push('clear browsing history');
    const browsers = privacyBrowsers(c).length;
    if (what.length === 0) what.push('clear browser data');
    parts.push(`${what.join(' and ')}${browsers > 0 ? ` from ${count(browsers, 'browser', 'browsers')}` : ''}`);
  }
  if (p.space) {
    const bytes = categoryOf(report, 'space')?.metrics.bytes ?? 0;
    parts.push(`delete ${formatBytes(bytes)} of junk`);
  }
  if (p.startupIds?.length) parts.push(`disable ${count(p.startupIds.length, 'startup item', 'startup items')}`);
  if (p.sleepAppIds?.length) parts.push(`put ${count(p.sleepAppIds.length, 'app', 'apps')} to sleep`);
  if (p.updateIds?.length) parts.push(`install ${count(p.updateIds.length, 'update', 'updates')}`);
  const text = parts.join(', ');
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** The full confirmation message: the summary plus the cautions that apply. */
export function confirmMessage(report: HealthReport, p: FixParams): string {
  const notes: string[] = [];
  if (p.privacy || p.space) notes.push('Deleted files cannot be restored.');
  if (p.sleepAppIds?.length) notes.push('Apps put to sleep are asked to quit, so save your work first.');
  if (p.updateIds?.length) notes.push('Installing updates may ask for administrator permission.');
  return [`${describeFix(report, p)}.`, ...notes].join(' ');
}

// ---------------------------------------------------------------- look

export type Tone = 'ok' | 'warn' | 'danger' | 'muted';

export function statusTone(s: HealthStatus): Tone {
  return s === 'good' ? 'ok' : s === 'warning' ? 'warn' : s === 'problem' ? 'danger' : 'muted';
}

export const STATUS_LABEL: Record<HealthStatus, string> = {
  good: 'Good',
  warning: 'Warning',
  problem: 'Problem',
  unavailable: 'Unavailable',
};

export function scoreTone(score: number | null): Tone {
  if (score === null) return 'muted';
  return score >= 80 ? 'ok' : score >= 50 ? 'warn' : 'danger';
}

export function scoreLabel(score: number | null): string {
  if (score === null) return 'Not measured';
  return score >= 90 ? 'Excellent' : score >= 75 ? 'Good' : score >= 50 ? 'Fair' : 'Poor';
}

/** "Fair - 2 areas need attention" */
export function statusLine(report: HealthReport): string {
  const measured = report.categories.filter((c) => c.status !== 'unavailable');
  if (report.score === null || measured.length === 0) return 'Could not measure your PC';
  const issues = measured.filter((c) => c.fixable).length;
  const label = scoreLabel(report.score);
  return issues === 0 ? `${label} - nothing needs attention` : `${label} - ${count(issues, 'area needs', 'areas need')} attention`;
}

export function ringAriaLabel(score: number | null): string {
  return score === null ? 'Health score not measured yet' : `Health score ${score} of 100`;
}

// ---------------------------------------------------------------- fix results

export const PART_LABEL: Record<FixPart, string> = {
  privacy: 'Privacy',
  space: 'Space',
  startup: 'Startup items',
  sleep: 'Background apps',
  updates: 'Updates',
};

function okCount(p: PartResult): number {
  return p.items.filter((i) => i.ok).length;
}

/** One line for the "What was done" list. */
export function describePart(p: PartResult): string {
  if (p.status === 'not_run') return 'Not run';
  let text: string;
  switch (p.part) {
    case 'privacy':
      text =
        p.removedRows + p.removedFiles === 0
          ? 'Nothing removed'
          : `Removed ${count(p.removedRows, 'browser entry', 'browser entries')}${
              p.removedFiles > 0 ? ` and ${count(p.removedFiles, 'file', 'files')}` : ''
            }`;
      break;
    case 'space':
      text =
        p.removedBytes + p.removedFiles + p.removedRows === 0
          ? 'Nothing deleted'
          : `Deleted ${formatBytes(p.removedBytes)} (${count(p.removedFiles, 'file', 'files')})`;
      break;
    case 'startup':
      text = `Switched off ${okCount(p)} of ${count(p.items.length, 'startup item', 'startup items')}`;
      break;
    case 'sleep':
      text = `Put ${okCount(p)} of ${count(p.items.length, 'app', 'apps')} to sleep`;
      break;
    case 'updates':
      text = `Installed ${okCount(p)} of ${count(p.items.length, 'update', 'updates')}`;
      break;
  }
  if (p.blockedApps.length > 0) text += `; ${p.blockedApps.join(', ')} still running, so it was skipped`;
  if (p.status === 'failed' && p.items.length === 0 && p.blockedApps.length === 0) text = p.message;
  return text;
}

/** Failed rows of a part, for the expandable detail. */
export function failedItems(p: PartResult) {
  return p.items.filter((i) => !i.ok);
}

/** Browsers that were running: (privacy / space parts that were skipped for that reason). */
export function blockedBrowsers(f: FixReport): string[] {
  const set = new Set<string>();
  for (const p of f.parts) for (const a of p.blockedApps) set.add(a);
  return [...set];
}

/** The retry for a running-browser skip: only the affected parts, closing the browsers. */
export function retryParams(f: FixReport): FixParams | null {
  const privacy = f.parts.some((p) => p.part === 'privacy' && p.blockedApps.length > 0);
  const space = f.parts.some((p) => p.part === 'space' && p.blockedApps.length > 0);
  if (!privacy && !space) return null;
  const p: FixParams = { closeApps: 'always' };
  if (privacy) p.privacy = true;
  if (space) p.space = true;
  return p;
}

/** Fold a retry into the first result: retried parts are replaced (their totals added up). */
export function mergeFixReports(prev: FixReport, next: FixReport): FixReport {
  const parts = prev.parts.map((p) => {
    const n = next.parts.find((x) => x.part === p.part);
    if (!n) return p;
    return {
      ...n,
      removedBytes: p.removedBytes + n.removedBytes,
      removedFiles: p.removedFiles + n.removedFiles,
      removedRows: p.removedRows + n.removedRows,
    };
  });
  for (const n of next.parts) if (!prev.parts.some((p) => p.part === n.part)) parts.push(n);
  return { parts, cancelled: next.cancelled, report: next.report };
}

/** Headline of the result card. */
export function fixHeadline(f: FixReport): string {
  const bad = f.parts.filter((p) => p.status === 'failed' || p.status === 'partial').length;
  if (bad === 0) return 'All done';
  return bad === f.parts.length ? 'Nothing could be fixed' : 'Done, with some issues';
}
