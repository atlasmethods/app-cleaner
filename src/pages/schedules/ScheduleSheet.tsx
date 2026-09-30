import { useEffect, useMemo, useState } from 'react';
import type { GroupInfo, RulesListing } from '../../api/cleaner';
import type { Schedule } from '../../api/scheduler';
import { BottomSheet } from '../../components/BottomSheet';
import { Checkbox } from '../../components/Checkbox';
import { ErrorBanner } from '../../components/ErrorBanner';
import {
  FREQUENCIES,
  WEEKDAYS,
  draftFrom,
  draftProblem,
  emptyDraft,
  slug,
  toInput,
  type Draft,
} from '../../lib/schedule';
import { ApiCallError, call } from '../../lib/transport';

interface Props {
  /** The schedule being edited, or null for a new one. */
  editing: Schedule | null;
  others: Schedule[];
  onClose: () => void;
  /** Called with the saved schedule. */
  onSaved: (s: Schedule) => void;
}

const field = 'h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 px-2 text-sm';

function allGroups(l: RulesListing): GroupInfo[] {
  return l.categories.flatMap((c) => c.groups);
}

export function ScheduleSheet({ editing, others, onClose, onSaved }: Props) {
  const [draft, setDraft] = useState<Draft>(() => (editing ? draftFrom(editing) : emptyDraft()));
  const [groups, setGroups] = useState<GroupInfo[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [error, setError] = useState<ApiCallError | null>(null);
  const [saving, setSaving] = useState(false);

  // The rule list is only needed to pick specific things to clean.
  const custom = draft.rules !== null;
  useEffect(() => {
    if (!custom || groups) return;
    let live = true;
    call<RulesListing>('cleaner.list_rules')
      .then((l) => live && setGroups(allGroups(l)))
      .catch((e) => live && setError(ApiCallError.from(e)));
    return () => {
      live = false;
    };
  }, [custom, groups]);

  const set = (p: Partial<Draft>) => setDraft((d) => ({ ...d, ...p }));
  const selected = useMemo(() => new Set(draft.rules ?? []), [draft.rules]);
  const minute = Number(draft.time.slice(3, 5)) || 0;

  const toggleGroup = (g: GroupInfo, on: boolean) => {
    const ids = g.rules.map((r) => r.id);
    const next = new Set(selected);
    for (const id of ids) {
      if (on) next.add(id);
      else next.delete(id);
    }
    set({ rules: [...next] });
  };

  const save = async () => {
    const p = draftProblem(draft, others, editing?.id);
    setProblem(p);
    if (p) return;
    setSaving(true);
    setError(null);
    try {
      const input = toInput(draft);
      const saved = editing
        ? await call<Schedule>('scheduler.update', { id: editing.id, ...input, enabled: editing.enabled })
        : await call<Schedule>('scheduler.add', input);
      onSaved(saved);
    } catch (e) {
      setError(ApiCallError.from(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <BottomSheet
      open
      title={editing ? 'Edit schedule' : 'Add schedule'}
      onClose={onClose}
      testId="schedule-sheet"
    >
      <ErrorBanner error={error} />
      <label className="block text-sm">
        <span className="mb-1 block font-medium">Name</span>
        <input
          type="text"
          value={draft.name}
          maxLength={80}
          onChange={(e) => set({ name: e.target.value })}
          data-testid="schedule-name"
          className={field}
        />
      </label>

      <label className="block text-sm">
        <span className="mb-1 block font-medium">How often</span>
        <select
          value={draft.frequency}
          onChange={(e) => set({ frequency: e.target.value as Draft['frequency'] })}
          data-testid="schedule-frequency"
          className={field}
        >
          {FREQUENCIES.map((f) => (
            <option key={f.id} value={f.id}>
              {f.label}
            </option>
          ))}
        </select>
      </label>

      {draft.frequency === 'weekly' && (
        <div role="group" aria-label="Weekdays" className="flex flex-wrap gap-1.5">
          {WEEKDAYS.map((name, i) => {
            const day = i + 1;
            const on = draft.weekdays.includes(day);
            return (
              <button
                key={day}
                type="button"
                aria-pressed={on}
                onClick={() =>
                  set({ weekdays: on ? draft.weekdays.filter((d) => d !== day) : [...draft.weekdays, day] })
                }
                data-testid={`schedule-weekday-${day}`}
                className={`h-9 min-w-10 rounded-full border px-2 text-xs font-medium ${
                  on ? 'border-accent bg-accent/10 text-accent-strong' : 'border-line bg-surface-2'
                }`}
              >
                {name}
              </button>
            );
          })}
        </div>
      )}

      {draft.frequency === 'monthly' && (
        <label className="block text-sm">
          <span className="mb-1 block font-medium">Day of the month</span>
          <select
            value={draft.dayOfMonth}
            onChange={(e) => set({ dayOfMonth: Number(e.target.value) })}
            data-testid="schedule-dom"
            className={field}
          >
            {Array.from({ length: 28 }, (_, i) => i + 1).map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
        </label>
      )}

      {(draft.frequency === 'daily' || draft.frequency === 'weekly' || draft.frequency === 'monthly') && (
        <label className="block text-sm">
          <span className="mb-1 block font-medium">Time</span>
          <input
            type="time"
            value={draft.time}
            onChange={(e) => set({ time: e.target.value })}
            data-testid="schedule-time"
            className={field}
          />
        </label>
      )}

      {draft.frequency === 'hourly' && (
        <label className="block text-sm">
          <span className="mb-1 block font-medium">Minutes past the hour</span>
          <input
            type="number"
            inputMode="numeric"
            min={0}
            max={59}
            value={minute}
            onChange={(e) => {
              const m = Math.min(59, Math.max(0, Math.trunc(Number(e.target.value) || 0)));
              set({ time: `00:${String(m).padStart(2, '0')}` });
            }}
            data-testid="schedule-minute"
            className={field}
          />
        </label>
      )}

      <fieldset className="m-0 min-w-0 border-0 p-0 text-sm">
        <legend className="mb-1 p-0 font-medium">What to clean</legend>
        <label className="flex min-h-9 items-center gap-2">
          <input
            type="radio"
            name="schedule-what"
            checked={!custom}
            onChange={() => set({ rules: null })}
            data-testid="schedule-rules-default"
          />
          <span className="min-w-0 break-words">The items selected on the Clean page</span>
        </label>
        <label className="flex min-h-9 items-center gap-2">
          <input
            type="radio"
            name="schedule-what"
            checked={custom}
            onChange={() => set({ rules: draft.rules ?? [] })}
            data-testid="schedule-rules-custom"
          />
          <span className="min-w-0 break-words">Only specific items</span>
        </label>
        {custom && (
          <ul className="m-0 mt-1 flex max-h-48 list-none flex-col gap-0.5 overflow-y-auto p-0" data-testid="schedule-rule-groups">
            {groups === null && <li className="text-xs text-muted">Loading...</li>}
            {groups?.map((g) => {
              const n = g.rules.filter((r) => selected.has(r.id)).length;
              return (
                <li key={g.group}>
                  <label className="flex min-h-9 min-w-0 items-center gap-2">
                    <Checkbox
                      checked={n === g.rules.length}
                      indeterminate={n > 0 && n < g.rules.length}
                      onChange={(on) => toggleGroup(g, on)}
                      testId={`schedule-rule-group-${slug(g.group)}`}
                      ariaLabel={g.group}
                    />
                    <span className="min-w-0 break-words">{g.group}</span>
                  </label>
                </li>
              );
            })}
          </ul>
        )}
      </fieldset>

      {problem && (
        <p className="m-0 text-xs text-danger" role="alert" data-testid="schedule-problem">
          {problem}
        </p>
      )}

      <div className="flex gap-2">
        <button
          type="button"
          onClick={onClose}
          className="h-10 flex-1 rounded-xl border border-line bg-surface-2 text-sm font-medium"
        >
          Cancel
        </button>
        <button
          type="button"
          disabled={saving}
          onClick={() => void save()}
          data-testid="schedule-save"
          className="h-10 flex-1 rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg disabled:opacity-60"
        >
          {saving ? 'Saving...' : 'Save'}
        </button>
      </div>
    </BottomSheet>
  );
}
