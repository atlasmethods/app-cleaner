import { Plus, X } from 'lucide-react';
import { useState } from 'react';
import type { MatchBy } from '../../api/duplicates';
import { BottomSheet } from '../../components/BottomSheet';
import { Checkbox } from '../../components/Checkbox';
import type { DupOptions } from '../../lib/dupOptions';

interface Props {
  open: boolean;
  options: DupOptions;
  onChange: (next: DupOptions) => void;
  onClose: () => void;
}

function PathList({
  label,
  items,
  onChange,
  testId,
  placeholder,
}: {
  label: string;
  items: string[];
  onChange: (next: string[]) => void;
  testId: string;
  placeholder: string;
}) {
  const [text, setText] = useState('');
  const add = () => {
    const p = text.trim();
    if (!p || items.includes(p)) return;
    onChange([...items, p]);
    setText('');
  };
  return (
    <div className="flex flex-col gap-2">
      <p className="m-0 text-xs font-semibold uppercase tracking-wide text-muted">{label}</p>
      <ul className="m-0 flex list-none flex-col gap-1 p-0" data-testid={`${testId}-list`}>
        {items.map((p) => (
          <li key={p} className="flex min-w-0 items-center gap-2 rounded-lg bg-surface-2 py-1 pl-3 pr-1">
            <span className="min-w-0 flex-1 break-all text-sm">{p}</span>
            <button
              type="button"
              onClick={() => onChange(items.filter((x) => x !== p))}
              aria-label={`Remove ${p}`}
              data-testid={`${testId}-remove`}
              className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border-0 bg-transparent"
            >
              <X size={16} aria-hidden />
            </button>
          </li>
        ))}
      </ul>
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          add();
        }}
      >
        <input
          type="text"
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder={placeholder}
          aria-label={label}
          data-testid={`${testId}-input`}
          className="h-10 min-w-0 flex-1 rounded-xl border border-line bg-surface-2 px-3 text-sm"
        />
        <button
          type="submit"
          disabled={!text.trim()}
          aria-label={`Add to ${label}`}
          data-testid={`${testId}-add`}
          className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-line bg-surface-2 disabled:opacity-50"
        >
          <Plus size={16} aria-hidden />
        </button>
      </form>
    </div>
  );
}

function Toggle({
  label,
  hint,
  checked,
  onChange,
  testId,
}: {
  label: string;
  hint?: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  testId: string;
}) {
  return (
    <label className="flex min-h-10 min-w-0 cursor-pointer items-center gap-3">
      <Checkbox checked={checked} onChange={onChange} testId={testId} ariaLabel={label} />
      <span className="min-w-0 flex-1 text-sm">
        {label}
        {hint && <span className="block text-[11px] text-muted">{hint}</span>}
      </span>
    </label>
  );
}

export function OptionsSheet({ open, options, onChange, onClose }: Props) {
  const set = <K extends keyof DupOptions>(k: K, v: DupOptions[K]) => onChange({ ...options, [k]: v });
  const setMatch = (k: keyof MatchBy, v: boolean) => set('match', { ...options.match, [k]: v });
  return (
    <BottomSheet open={open} title="Search options" onClose={onClose} testId="dup-options">
      <div className="flex flex-col gap-4">
        <PathList
          label="Folders to search"
          items={options.paths}
          onChange={(v) => set('paths', v)}
          testId="dup-path"
          placeholder="Type a folder path"
        />
        <PathList
          label="Folders to skip"
          items={options.exclude}
          onChange={(v) => set('exclude', v)}
          testId="dup-exclude"
          placeholder="Folder or file to skip"
        />

        <div className="flex flex-col">
          <p className="m-0 mb-1 text-xs font-semibold uppercase tracking-wide text-muted">Match files by</p>
          <Toggle label="Content" hint="Byte-for-byte identical (recommended)" checked={options.match.content} onChange={(v) => setMatch('content', v)} testId="dup-match-content" />
          <Toggle label="Name" hint="Same file name" checked={options.match.name} onChange={(v) => setMatch('name', v)} testId="dup-match-name" />
          <Toggle label="Size" checked={options.match.size} onChange={(v) => setMatch('size', v)} testId="dup-match-size" />
          <Toggle label="Modified date" checked={options.match.modified} onChange={(v) => setMatch('modified', v)} testId="dup-match-modified" />
          {!options.match.content && (
            <p className="m-0 mt-1 text-xs text-warn" data-testid="dup-weak-warning">
              Without content matching, files can look alike but differ. Check them before deleting.
            </p>
          )}
        </div>

        <div className="flex flex-col gap-2">
          <p className="m-0 text-xs font-semibold uppercase tracking-wide text-muted">File size (MB)</p>
          <div className="flex gap-2">
            <label className="min-w-0 flex-1 text-xs text-muted">
              At least
              <input
                type="number"
                inputMode="decimal"
                min={0}
                value={options.minMb}
                onChange={(e) => set('minMb', e.target.value)}
                data-testid="dup-min"
                className="mt-1 h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 px-3 text-sm text-fg"
              />
            </label>
            <label className="min-w-0 flex-1 text-xs text-muted">
              At most
              <input
                type="number"
                inputMode="decimal"
                min={0}
                value={options.maxMb}
                onChange={(e) => set('maxMb', e.target.value)}
                data-testid="dup-max"
                className="mt-1 h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 px-3 text-sm text-fg"
              />
            </label>
          </div>
        </div>

        <div className="flex flex-col">
          <Toggle label="Skip empty files" checked={options.skipZero} onChange={(v) => set('skipZero', v)} testId="dup-skipzero" />
          <Toggle label="Include hidden files and folders" checked={options.hidden} onChange={(v) => set('hidden', v)} testId="dup-hidden" />
          <Toggle
            label="Include system files"
            hint="Operating system folders can be listed but never deleted"
            checked={options.system}
            onChange={(v) => set('system', v)}
            testId="dup-system"
          />
        </div>

        <button
          type="button"
          onClick={onClose}
          data-testid="dup-options-done"
          className="h-10 rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg"
        >
          Done
        </button>
      </div>
    </BottomSheet>
  );
}
