import { describe, expect, it } from 'vitest';
import type { OptimizerApp } from '../api/optimizer';
import { activeApps, appSubline, sleepable, sleepingApps, sleepReport, summarize, wakeReport } from './optimizer';

const MB = 1024 * 1024;
const app = (over: Partial<OptimizerApp> & { appId: string }): OptimizerApp => ({
  name: over.appId,
  processes: [],
  startupIds: [],
  serviceIds: [],
  backgroundMemoryBytes: 0,
  cpuPercent: 0,
  sleeping: false,
  protected: false,
  ...over,
});
const proc = (pid: number, mb: number) => ({ pid, name: 'p', memoryBytes: mb * MB, cpuPercent: 1 });

const APPS = [
  app({ appId: 'small', processes: [proc(1, 10)], backgroundMemoryBytes: 10 * MB }),
  app({ appId: 'big', processes: [proc(2, 300)], backgroundMemoryBytes: 300 * MB, startupIds: ['x'] }),
  app({ appId: 'av', processes: [proc(3, 50)], backgroundMemoryBytes: 50 * MB, protected: true }),
  app({ appId: 'zzz', sleeping: true }),
  app({ appId: 'idle', startupIds: ['a', 'b'] }),
];

describe('optimizer helpers', () => {
  it('orders active apps by memory and splits sleeping ones', () => {
    expect(activeApps(APPS).map((a) => a.appId)).toEqual(['big', 'av', 'small', 'idle']);
    expect(sleepingApps(APPS).map((a) => a.appId)).toEqual(['zzz']);
  });

  it('never offers protected apps for "sleep all"', () => {
    expect(sleepable(APPS).map((a) => a.appId)).toEqual(['big', 'small', 'idle']);
  });

  it('summarises running apps and their memory', () => {
    const s = summarize(APPS);
    expect(s.runningApps).toBe(3);
    expect(s.memoryBytes).toBe(360 * MB);
    expect(s.memoryLabel).toBe('360 MB');
    expect(summarize([]).memoryLabel).toBe('0 B');
  });

  it('describes rows', () => {
    expect(appSubline(APPS[1]!)).toBe('300 MB - 0.0% CPU - 1 process - 1 startup item');
    expect(appSubline(APPS[4]!)).toBe('Not running - 2 startup items');
  });

  it('reports sleep and wake outcomes honestly', () => {
    const r = sleepReport([
      { appId: 'a', name: 'A', ok: true, stoppedProcesses: 3, stillRunning: 1 },
      { appId: 'b', name: 'B', ok: false, error: 'protected' },
    ]);
    expect(r.ok).toBe(false);
    expect(r.text).toContain('1 app put to sleep, 3 processes closed.');
    expect(r.text).toContain('1 process is still running');
    expect(r.text).toContain('B: protected');
    expect(sleepReport([{ appId: 'a', name: 'A', ok: true, stoppedProcesses: 1 }])).toEqual({
      ok: true,
      text: '1 app put to sleep, 1 process closed.',
    });
    const w = wakeReport([{ appId: 'a', name: 'A', ok: true, missingItems: ['X'] }]);
    expect(w.ok).toBe(true);
    expect(w.text).toContain('1 startup item no longer existed');
  });
});
