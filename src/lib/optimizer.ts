import type { OptimizerApp, SleepResult, WakeResult } from '../api/optimizer';
import { formatBytes } from './format';

/** Running / startup apps that could be put to sleep, biggest memory first. */
export function activeApps(apps: OptimizerApp[]): OptimizerApp[] {
  return apps
    .filter((a) => !a.sleeping)
    .sort((a, b) => b.backgroundMemoryBytes - a.backgroundMemoryBytes || a.name.localeCompare(b.name));
}

export function sleepingApps(apps: OptimizerApp[]): OptimizerApp[] {
  return apps.filter((a) => a.sleeping).sort((a, b) => a.name.localeCompare(b.name));
}

/** Apps "Sleep all" would act on: not protected and not asleep already. */
export function sleepable(apps: OptimizerApp[]): OptimizerApp[] {
  return activeApps(apps).filter((a) => !a.protected);
}

export interface Summary {
  runningApps: number;
  memoryBytes: number;
  memoryLabel: string;
}

/** Headline numbers for the summary card: only apps that are not asleep and run something. */
export function summarize(apps: OptimizerApp[]): Summary {
  const running = activeApps(apps).filter((a) => a.processes.length > 0);
  const memoryBytes = running.reduce((s, a) => s + a.backgroundMemoryBytes, 0);
  return { runningApps: running.length, memoryBytes, memoryLabel: formatBytes(memoryBytes) };
}

/** One line under an app row: memory / CPU, or what starts automatically. */
export function appSubline(a: OptimizerApp): string {
  const parts: string[] = [];
  if (a.processes.length > 0) {
    parts.push(`${formatBytes(a.backgroundMemoryBytes)}`);
    parts.push(`${a.cpuPercent.toFixed(1)}% CPU`);
    parts.push(`${a.processes.length} ${a.processes.length === 1 ? 'process' : 'processes'}`);
  } else {
    parts.push('Not running');
  }
  const startup = a.startupIds.length + a.serviceIds.length;
  if (startup > 0) parts.push(`${startup} startup ${startup === 1 ? 'item' : 'items'}`);
  return parts.join(' - ');
}

/** Text for the result note after sleeping apps; empty when everything went cleanly. */
export function sleepReport(results: SleepResult[]): { ok: boolean; text: string } {
  const bad = results.filter((r) => !r.ok);
  const stuck = results.reduce((s, r) => s + (r.stillRunning ?? 0), 0);
  const stopped = results.reduce((s, r) => s + (r.stoppedProcesses ?? 0), 0);
  const okCount = results.length - bad.length;
  let text = `${okCount} ${okCount === 1 ? 'app' : 'apps'} put to sleep`;
  if (stopped > 0) text += `, ${stopped} ${stopped === 1 ? 'process' : 'processes'} closed`;
  text += '.';
  if (stuck > 0) text += ` ${stuck} ${stuck === 1 ? 'process is' : 'processes are'} still running; close the app yourself.`;
  if (bad.length > 0) {
    const first = bad[0]!;
    text += ` ${bad.length} could not be changed: ${first.name}: ${first.error ?? first.errors?.[0] ?? 'unknown error'}`;
  }
  return { ok: bad.length === 0 && stuck === 0, text };
}

export function wakeReport(results: WakeResult[]): { ok: boolean; text: string } {
  const bad = results.filter((r) => !r.ok);
  const okCount = results.length - bad.length;
  const missing = results.reduce((s, r) => s + (r.missingItems?.length ?? 0), 0);
  let text = `${okCount} ${okCount === 1 ? 'app' : 'apps'} woken. Its startup items are back the way they were.`;
  if (missing > 0) text += ` ${missing} startup ${missing === 1 ? 'item' : 'items'} no longer existed.`;
  if (bad.length > 0) text += ` ${bad.length} could not be changed: ${bad[0]!.error ?? bad[0]!.errors?.[0] ?? 'unknown error'}`;
  return { ok: bad.length === 0, text };
}
