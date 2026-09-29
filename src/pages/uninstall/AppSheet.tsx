import { useState } from 'react';
import { PackageX, Pencil, Trash2, Wrench, XCircle } from 'lucide-react';
import type { AppEntry } from '../../api/uninstall';
import { BottomSheet } from '../../components/BottomSheet';
import { actionsFor, sourceLabel, subtitle } from '../../lib/apps';
import { formatBytes } from '../../lib/format';

interface Props {
  app: AppEntry | null;
  onClose: () => void;
  onUninstall: (app: AppEntry) => void;
  onRepair: (app: AppEntry) => void;
  onRename: (app: AppEntry, name: string) => void;
  onRemoveEntry: (app: AppEntry) => void;
}

const btn =
  'flex h-11 w-full items-center gap-2 rounded-xl border border-line bg-surface-2 px-3 text-left text-sm font-medium';

/** Row menu: only the actions that exist for this entry are shown. */
export function AppSheet({ app, onClose, onUninstall, onRepair, onRename, onRemoveEntry }: Props) {
  const [renaming, setRenaming] = useState(false);
  const [name, setName] = useState('');
  const close = () => {
    setRenaming(false);
    onClose();
  };
  if (!app) return null;
  const a = actionsFor(app);
  const facts = [
    sourceLabel(app.source),
    app.sizeBytes !== undefined ? formatBytes(app.sizeBytes) : null,
    app.installDate ?? null,
  ]
    .filter(Boolean)
    .join(' - ');

  return (
    <BottomSheet
      open
      title={app.name}
      subtitle={[subtitle(app), facts].filter(Boolean).join(' | ')}
      onClose={close}
      testId="app-sheet"
    >
      {app.isSystem && (
        <p className="m-0 rounded-lg border border-warn/40 bg-warn/10 p-2 text-xs" data-testid="app-sheet-system">
          System component. Removing it may break your system.
        </p>
      )}
      {!renaming ? (
        <>
          {a.uninstall && (
            <button type="button" className={`${btn} !border-0 !bg-danger !text-white`} onClick={() => onUninstall(app)} data-testid="app-action-uninstall">
              <Trash2 size={16} aria-hidden /> Uninstall
            </button>
          )}
          {!a.uninstall && (
            <p className="m-0 text-xs text-muted" data-testid="app-sheet-no-uninstaller">
              This entry has no uninstaller.
            </p>
          )}
          {a.repair && (
            <button type="button" className={btn} onClick={() => onRepair(app)} data-testid="app-action-repair">
              <Wrench size={16} aria-hidden /> Repair
            </button>
          )}
          {a.renameEntry && (
            <button
              type="button"
              className={btn}
              onClick={() => {
                setName(app.name);
                setRenaming(true);
              }}
              data-testid="app-action-rename"
            >
              <Pencil size={16} aria-hidden /> Rename entry
            </button>
          )}
          {a.removeEntry && (
            <button type="button" className={btn} onClick={() => onRemoveEntry(app)} data-testid="app-action-remove-entry">
              <XCircle size={16} aria-hidden /> Delete entry
            </button>
          )}
        </>
      ) : (
        <form
          className="flex flex-col gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (name.trim()) onRename(app, name.trim());
          }}
        >
          <label className="text-xs text-muted" htmlFor="app-rename-input">
            New name in the programs list
          </label>
          <input
            id="app-rename-input"
            value={name}
            onChange={(e) => setName(e.target.value)}
            maxLength={200}
            data-testid="app-rename-input"
            className="h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 px-3 text-sm"
          />
          <div className="flex gap-2">
            <button type="button" className="h-10 flex-1 rounded-xl border border-line bg-surface-2 text-sm" onClick={() => setRenaming(false)}>
              Back
            </button>
            <button
              type="submit"
              disabled={!name.trim()}
              data-testid="app-rename-continue"
              className="h-10 flex-1 rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg disabled:opacity-50"
            >
              Continue
            </button>
          </div>
        </form>
      )}
      <p className="m-0 flex items-center gap-1 text-[11px] text-muted">
        <PackageX size={12} aria-hidden /> {app.id}
      </p>
    </BottomSheet>
  );
}
