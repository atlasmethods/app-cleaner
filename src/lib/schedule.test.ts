import { describe, expect, it } from 'vitest';
import type { Schedule } from '../api/scheduler';
import {
  agentLine,
  describeSchedule,
  draftFrom,
  draftProblem,
  emptyDraft,
  joinWords,
  toInput,
  type Draft,
} from './schedule';

const base = { time: '03:00', weekdays: [] as number[], dayOfMonth: 1 };

describe('describeSchedule', () => {
  it.each([
    [{ ...base, frequency: 'daily' as const }, 'Every day at 03:00'],
    [{ ...base, frequency: 'weekly' as const, weekdays: [1, 3] }, 'Every Mon and Wed at 03:00'],
    [{ ...base, frequency: 'weekly' as const, weekdays: [5, 1, 3] }, 'Every Mon, Wed and Fri at 03:00'],
    [{ ...base, frequency: 'weekly' as const, weekdays: [7] }, 'Every Sun at 03:00'],
    [{ ...base, frequency: 'weekly' as const, weekdays: [1, 2, 3, 4, 5, 6, 7] }, 'Every day at 03:00'],
    [{ ...base, frequency: 'weekly' as const, weekdays: [2, 2] }, 'Every Tue at 03:00'],
    [{ ...base, frequency: 'weekly' as const }, 'Weekly at 03:00'],
    [{ ...base, frequency: 'monthly' as const, dayOfMonth: 15, time: '21:45' }, 'On day 15 of every month at 21:45'],
    [{ ...base, frequency: 'hourly' as const, time: '00:00' }, 'Every hour, on the hour'],
    [{ ...base, frequency: 'hourly' as const, time: '09:20' }, 'Every hour, 20 minutes past'],
    [{ ...base, frequency: 'on_login' as const }, 'Every time you log in'],
  ])('%j', (s, want) => {
    expect(describeSchedule(s)).toBe(want);
  });

  it('joins words', () => {
    expect(joinWords([])).toBe('');
    expect(joinWords(['a'])).toBe('a');
    expect(joinWords(['a', 'b'])).toBe('a and b');
    expect(joinWords(['a', 'b', 'c'])).toBe('a, b and c');
  });
});

describe('drafts', () => {
  const others = [{ id: 'other', name: 'Nightly' }];
  const ok = (over: Partial<Draft> = {}): Draft => ({ ...emptyDraft(), name: 'Mine', ...over });

  it('accepts a good draft', () => {
    expect(draftProblem(ok(), others)).toBeNull();
    expect(draftProblem(ok({ frequency: 'weekly', weekdays: [2, 4] }), others)).toBeNull();
    expect(draftProblem(ok({ frequency: 'monthly', dayOfMonth: 28 }), others)).toBeNull();
    expect(draftProblem(ok({ rules: ['chrome.cache'] }), others)).toBeNull();
  });

  it.each([
    [{ name: '  ' }, 'name'],
    [{ name: 'x'.repeat(61) }, '60 characters'],
    [{ name: 'nightly' }, 'already a schedule'],
    [{ time: '3:00' }, 'HH:MM'],
    [{ time: '24:00' }, 'HH:MM'],
    [{ time: '12:60' }, 'HH:MM'],
    [{ frequency: 'weekly' as const, weekdays: [] }, 'weekday'],
    [{ frequency: 'monthly' as const, dayOfMonth: 29 }, 'day of the month'],
    [{ frequency: 'monthly' as const, dayOfMonth: 0 }, 'day of the month'],
    [{ rules: [] }, 'at least one thing'],
  ])('rejects %j', (over, needle) => {
    expect(draftProblem(ok(over), others)).toContain(needle);
  });

  it('lets a schedule keep its own name when editing', () => {
    expect(draftProblem(ok({ name: 'Nightly' }), others, 'other')).toBeNull();
  });

  it('round-trips a schedule through a draft into an input', () => {
    const s: Schedule = {
      id: 'x',
      name: 'Weekly',
      enabled: false,
      frequency: 'weekly',
      time: '21:30',
      weekdays: [3, 1],
      dayOfMonth: 4,
      action: { kind: 'clean', rules: ['a'] },
      createdAt: '2024-01-01T00:00:00Z',
    };
    expect(toInput({ ...draftFrom(s), name: '  Weekly ' })).toEqual({
      name: 'Weekly',
      enabled: false,
      frequency: 'weekly',
      time: '21:30',
      weekdays: [1, 3],
      dayOfMonth: 4,
      action: { kind: 'clean', rules: ['a'] },
    });
    expect(draftFrom({ ...s, weekdays: [] }).weekdays).toEqual([1]);
  });
});

describe('agentLine', () => {
  const when = (iso: string) => `@${iso}`;
  it('words the agent state', () => {
    expect(agentLine(null, when)).toContain('Checking');
    expect(agentLine({ running: true, since: '2024-01-01T10:00:00Z' }, when)).toBe(
      'Background agent running since @2024-01-01T10:00:00Z',
    );
    expect(agentLine({ running: true }, when)).toBe('Background agent running');
    expect(agentLine({ running: false }, when)).toBe('Not running — enable Run at startup or keep ClearSweep open');
  });
});
