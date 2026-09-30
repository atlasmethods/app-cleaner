import type { Frequency, Schedule, ScheduleInput } from '../api/scheduler';

/** `Google Chrome` -> `google-chrome` (test ids and keys). */
export function slug(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
}

export const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'] as const;

export const FREQUENCIES: { id: Frequency; label: string }[] = [
  { id: 'daily', label: 'Daily' },
  { id: 'weekly', label: 'Weekly' },
  { id: 'monthly', label: 'Monthly' },
  { id: 'hourly', label: 'Hourly' },
  { id: 'on_login', label: 'At login' },
];

/** "Mon, Wed and Fri" */
export function joinWords(words: string[]): string {
  if (words.length <= 1) return words.join('');
  return `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`;
}

/** The next-run rule in words: "Every Mon and Wed at 03:00". */
export function describeSchedule(s: Pick<Schedule, 'frequency' | 'time' | 'weekdays' | 'dayOfMonth'>): string {
  const minute = s.time.slice(3, 5);
  switch (s.frequency) {
    case 'daily':
      return `Every day at ${s.time}`;
    case 'weekly': {
      const days = [...new Set(s.weekdays)].filter((d) => d >= 1 && d <= 7).sort((a, b) => a - b);
      if (days.length === 0) return `Weekly at ${s.time}`;
      if (days.length === 7) return `Every day at ${s.time}`;
      return `Every ${joinWords(days.map((d) => WEEKDAYS[d - 1]!))} at ${s.time}`;
    }
    case 'monthly':
      return `On day ${s.dayOfMonth} of every month at ${s.time}`;
    case 'hourly':
      return minute === '00' ? 'Every hour, on the hour' : `Every hour, ${Number(minute)} minutes past`;
    case 'on_login':
      return 'Every time you log in';
  }
}

export interface Draft {
  name: string;
  frequency: Frequency;
  time: string;
  weekdays: number[];
  dayOfMonth: number;
  /** null = the rules enabled in Settings */
  rules: string[] | null;
  enabled: boolean;
}

export function emptyDraft(): Draft {
  return { name: '', frequency: 'daily', time: '03:00', weekdays: [1], dayOfMonth: 1, rules: null, enabled: true };
}

export function draftFrom(s: Schedule): Draft {
  return {
    name: s.name,
    frequency: s.frequency,
    time: s.time,
    weekdays: s.weekdays.length > 0 ? s.weekdays : [1],
    dayOfMonth: s.dayOfMonth,
    rules: s.action.rules,
    enabled: s.enabled,
  };
}

/** A problem with the draft in words, or null when it can be saved. */
export function draftProblem(d: Draft, others: Pick<Schedule, 'id' | 'name'>[], editingId?: string): string | null {
  const name = d.name.trim();
  if (!name) return 'Give the schedule a name.';
  if (name.length > 60) return 'The name can have at most 60 characters.';
  if (others.some((o) => o.id !== editingId && o.name.trim().toLowerCase() === name.toLowerCase()))
    return `There is already a schedule named "${name}".`;
  if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(d.time)) return 'Enter the time as HH:MM (24 hours).';
  if (d.frequency === 'weekly' && d.weekdays.length === 0) return 'Choose at least one weekday.';
  if (d.frequency === 'monthly' && (!Number.isInteger(d.dayOfMonth) || d.dayOfMonth < 1 || d.dayOfMonth > 28))
    return 'Choose a day of the month from 1 to 28.';
  if (d.rules !== null && d.rules.length === 0) return 'Choose at least one thing to clean, or use the Settings selection.';
  return null;
}

export function toInput(d: Draft): ScheduleInput {
  return {
    name: d.name.trim(),
    enabled: d.enabled,
    frequency: d.frequency,
    time: d.time,
    weekdays: [...d.weekdays].sort((a, b) => a - b),
    dayOfMonth: d.dayOfMonth,
    action: { kind: 'clean', rules: d.rules },
  };
}

export const SMART_THRESHOLD_PRESETS = [100, 500, 1000, 2000] as const;

/** "Background agent running since 10:15" style text for Settings. */
export function agentLine(
  status: { running: boolean; since?: string } | null,
  formatWhen: (iso: string) => string,
): string {
  if (!status) return 'Checking the background agent...';
  if (status.running) {
    return status.since ? `Background agent running since ${formatWhen(status.since)}` : 'Background agent running';
  }
  return 'Not running — enable Run at startup or keep ClearSweep open';
}
