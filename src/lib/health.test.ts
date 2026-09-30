import { describe, expect, it } from 'vitest';
import type { FixReport, HealthReport } from '../api/health';
import { fixReport, report100, report72, securityCat, speedCat, spaceCat, privacyCat } from '../test/healthFixtures';
import {
  blockedBrowsers,
  canFix,
  categoryOf,
  confirmMessage,
  defaultSelection,
  describeFix,
  describePart,
  fixHeadline,
  isEmptyFix,
  mergeFixReports,
  retryParams,
  ringAriaLabel,
  scoreLabel,
  scoreTone,
  statusLine,
  toFixParams,
  toggleIn,
} from './health';

describe('defaultSelection', () => {
  it('ticks every fixable finding except non-security updates', () => {
    const s = defaultSelection(report72);
    expect(s.privacy).toBe(true);
    expect(s.space).toBe(true);
    expect([...s.startup]).toEqual([
      'xdg:user:slack.desktop',
      'xdg:user:spotify.desktop',
      'xdg:user:dropbox.desktop',
      'xdg:user:steam.desktop',
    ]);
    expect([...s.apps]).toEqual(['slack', 'spotify']);
    expect([...s.updates]).toEqual(['apt:firefox']);
  });

  it('selects nothing in categories that are good or unavailable', () => {
    const s = defaultSelection(report100);
    expect(s.privacy || s.space).toBe(false);
    expect(s.startup.size + s.apps.size + s.updates.size).toBe(0);

    const unavailable: HealthReport = {
      ...report72,
      categories: [
        { ...privacyCat, status: 'unavailable', summary: 'boom' },
        spaceCat,
        { ...speedCat, status: 'unavailable', summary: 'boom' },
        { ...securityCat, status: 'unavailable', summary: 'No supported package manager' },
      ],
    };
    const u = defaultSelection(unavailable);
    expect(u.privacy).toBe(false);
    expect(u.space).toBe(true);
    expect(u.startup.size + u.apps.size + u.updates.size).toBe(0);
    expect(canFix(categoryOf(unavailable, 'privacy'))).toBe(false);
  });
});

describe('toFixParams', () => {
  it('sends only what is ticked and offered', () => {
    const sel = {
      ...defaultSelection(report72),
      space: false,
      startup: new Set(['xdg:user:slack.desktop', 'not-in-the-report']),
      apps: new Set<string>(),
      updates: new Set(['apt:vim', 'apt:ghost']),
    };
    expect(toFixParams(report72, sel)).toEqual({
      privacy: true,
      startupIds: ['xdg:user:slack.desktop'],
      updateIds: ['apt:vim'],
    });
  });

  it('is empty when nothing is selected, and never sends a part the report does not offer', () => {
    const none = {
      privacy: false,
      space: false,
      startup: new Set<string>(),
      apps: new Set<string>(),
      updates: new Set<string>(),
    };
    expect(isEmptyFix(toFixParams(report72, none))).toBe(true);
    // privacy ticked, but the (fresh) report says there is nothing to do for it
    expect(toFixParams(report100, { ...none, privacy: true, space: true })).toEqual({});
  });

  it('toggleIn adds and removes without mutating', () => {
    const a = new Set(['x']);
    const b = toggleIn(a, 'y');
    expect([...a]).toEqual(['x']);
    expect([...b]).toEqual(['x', 'y']);
    expect([...toggleIn(b, 'x')]).toEqual(['y']);
  });
});

describe('describeFix', () => {
  it('names exactly what will happen', () => {
    const sel = defaultSelection(report72);
    const params = { ...toFixParams(report72, sel), updateIds: ['apt:firefox', 'apt:vim', 'apt:htop'] };
    expect(describeFix(report72, params)).toBe(
      'Delete 1.2 GB of junk, remove tracking cookies from 3 browsers, disable 4 startup items, put 2 apps to sleep, install 3 updates',
    );
  });

  it('uses singular forms and only mentions selected parts', () => {
    expect(describeFix(report72, { startupIds: ['a'], sleepAppIds: ['b'], updateIds: ['c'] })).toBe(
      'Disable 1 startup item, put 1 app to sleep, install 1 update',
    );
    expect(describeFix(report72, { space: true })).toBe('Delete 1.2 GB of junk');
  });

  it('mentions history when the privacy findings include it', () => {
    const withHistory: HealthReport = {
      ...report72,
      categories: [
        {
          ...privacyCat,
          findings: [
            ...privacyCat.findings,
            { kind: 'history', count: 40, browsers: ['Google Chrome', 'Mozilla Firefox'] },
          ],
        },
        spaceCat,
        speedCat,
        securityCat,
      ],
    };
    expect(describeFix(withHistory, { privacy: true })).toBe(
      'Remove tracking cookies and clear browsing history from 3 browsers',
    );
  });

  it('adds cautions that apply to the selection', () => {
    expect(confirmMessage(report72, { space: true })).toBe('Delete 1.2 GB of junk. Deleted files cannot be restored.');
    const m = confirmMessage(report72, { sleepAppIds: ['slack'], updateIds: ['apt:vim'] });
    expect(m).toContain('save your work first');
    expect(m).toContain('administrator permission');
    expect(m).not.toContain('cannot be restored');
  });
});

describe('score presentation', () => {
  it('has an accessible label and tones', () => {
    expect(ringAriaLabel(72)).toBe('Health score 72 of 100');
    expect(ringAriaLabel(null)).toBe('Health score not measured yet');
    expect([scoreTone(95), scoreTone(80), scoreTone(79), scoreTone(50), scoreTone(49), scoreTone(null)]).toEqual([
      'ok',
      'ok',
      'warn',
      'warn',
      'danger',
      'muted',
    ]);
    expect([scoreLabel(100), scoreLabel(75), scoreLabel(60), scoreLabel(10)]).toEqual(['Excellent', 'Good', 'Fair', 'Poor']);
  });

  it('summarises how many areas need attention', () => {
    expect(statusLine(report72)).toBe('Fair - 4 areas need attention');
    expect(statusLine(report100)).toBe('Excellent - nothing needs attention');
    expect(statusLine({ ...report100, score: null, categories: [] })).toBe('Could not measure your PC');
    expect(
      statusLine({ ...report72, categories: [{ ...securityCat, status: 'unavailable', summary: 'x' }, spaceCat] }),
    ).toBe('Fair - 1 area needs attention');
  });
});

describe('fix results', () => {
  it('describes each part in one line', () => {
    const f = fixReport();
    expect(describePart(f.parts[0]!)).toBe('Removed 12 browser entries');
    expect(describePart(f.parts[1]!)).toBe('Deleted 1.2 GB (214 files)');
    expect(
      describePart({
        part: 'startup',
        status: 'partial',
        message: '',
        removedBytes: 0,
        removedFiles: 0,
        removedRows: 0,
        blockedApps: [],
        items: [
          { id: 'a', name: 'A', ok: true, message: '' },
          { id: 'b', name: 'B', ok: false, message: 'Refused' },
        ],
      }),
    ).toBe('Switched off 1 of 2 startup items');
    expect(
      describePart({ ...f.parts[1]!, status: 'partial', removedBytes: 10, removedFiles: 1, blockedApps: ['Google Chrome'] }),
    ).toBe('Deleted 10 B (1 file); Google Chrome still running, so it was skipped');
    expect(describePart({ ...f.parts[0]!, status: 'not_run' })).toBe('Not run');
    expect(describePart({ ...f.parts[0]!, status: 'failed', removedRows: 0, message: 'boom' })).toBe('boom');
  });

  it('finds the blocked browsers and builds the retry for only those parts', () => {
    const f = fixReport();
    expect(retryParams(f)).toBeNull();
    const blocked: FixReport = {
      ...f,
      parts: [
        { ...f.parts[0]!, status: 'failed', removedRows: 0, blockedApps: ['Google Chrome'] },
        { ...f.parts[1]!, status: 'partial', blockedApps: ['Google Chrome', 'Mozilla Firefox'] },
        {
          part: 'startup',
          status: 'done',
          message: '',
          removedBytes: 0,
          removedFiles: 0,
          removedRows: 0,
          blockedApps: [],
          items: [],
        },
      ],
    };
    expect(blockedBrowsers(blocked)).toEqual(['Google Chrome', 'Mozilla Firefox']);
    // the startup part is NOT repeated
    expect(retryParams(blocked)).toEqual({ closeApps: 'always', privacy: true, space: true });
    const onlySpace: FixReport = { ...blocked, parts: [{ ...blocked.parts[0]!, blockedApps: [] }, blocked.parts[1]!] };
    expect(retryParams(onlySpace)).toEqual({ closeApps: 'always', space: true });
  });

  it('merges a retry into the first result, adding up what was removed', () => {
    const first: FixReport = {
      ...fixReport(),
      report: report72,
      parts: [
        { ...fixReport().parts[0]!, status: 'failed', removedRows: 0, blockedApps: ['Google Chrome'] },
        { ...fixReport().parts[1]!, status: 'partial', removedBytes: 100, removedFiles: 2, blockedApps: ['Google Chrome'] },
        {
          part: 'updates',
          status: 'done',
          message: '',
          removedBytes: 0,
          removedFiles: 0,
          removedRows: 0,
          blockedApps: [],
          items: [{ id: 'apt:vim', name: 'vim', ok: true, message: '' }],
        },
      ],
    };
    const retry: FixReport = {
      ...fixReport(),
      parts: [
        { ...fixReport().parts[0]!, removedRows: 5 },
        { ...fixReport().parts[1]!, removedBytes: 50, removedFiles: 1 },
      ],
    };
    const m = mergeFixReports(first, retry);
    expect(m.parts.map((p) => [p.part, p.status])).toEqual([
      ['privacy', 'done'],
      ['space', 'done'],
      ['updates', 'done'],
    ]);
    expect(m.parts[0]!.removedRows).toBe(5);
    expect([m.parts[1]!.removedBytes, m.parts[1]!.removedFiles]).toEqual([150, 3]);
    expect(m.parts[2]!.items).toHaveLength(1);
    expect(m.report).toBe(retry.report);
    expect(retryParams(m)).toBeNull();
  });

  it('headlines the outcome', () => {
    const f = fixReport();
    expect(fixHeadline(f)).toBe('All done');
    expect(fixHeadline({ ...f, parts: [f.parts[0]!, { ...f.parts[1]!, status: 'partial' }] })).toBe('Done, with some issues');
    expect(fixHeadline({ ...f, parts: f.parts.map((p) => ({ ...p, status: 'failed' as const })) })).toBe(
      'Nothing could be fixed',
    );
  });
});
