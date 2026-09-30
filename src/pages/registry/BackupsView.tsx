import { ArrowLeft, RotateCcw, Trash2 } from 'lucide-react';
import { useEffect, useState } from 'react';
import type { BackupInfo, RestoreOutcome } from '../../api/registry_cleaner';
import { Card } from '../../components/Card';
import { ConfirmSheet } from '../../components/ConfirmSheet';
import { EmptyState } from '../../components/EmptyState';
import { ErrorBanner } from '../../components/ErrorBanner';
import { RunProgress } from '../../components/RunProgress';
import { useCall } from '../../hooks/useCall';
import { formatBytes, formatWhen } from '../../lib/format';
import { plural } from '../../lib/registry';

type Pending = { kind: 'restore' | 'delete'; backup: BackupInfo };

interface Props {
  onBack: () => void;
}

/** Backups made before fixes: restore or delete them. */
export function BackupsView({ onBack }: Props) {
  const list = useCall<BackupInfo[]>('registry_cleaner.list_backups');
  const restore = useCall<RestoreOutcome, { id: string }>('registry_cleaner.restore_backup');
  const del = useCall<{ id: string; freedBytes: number }, { id: string }>('registry_cleaner.delete_backup');
  const [pending, setPending] = useState<Pending | null>(null);
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);

  const load = list.run;
  useEffect(() => {
    void load();
  }, [load]);

  const busy = restore.loading || del.loading;
  const active = restore.loading ? restore : del.loading ? del : null;

  const confirm = async () => {
    const p = pending;
    if (!p) return;
    setPending(null);
    setNote(null);
    if (p.kind === 'restore') {
      const r = await restore.run({ id: p.backup.id });
      if (r) {
        const failed = r.results.filter((x) => !x.ok);
        setNote({
          ok: r.ok,
          text: r.ok
            ? `Restored ${plural(r.restored, 'item')}.`
            : `Restored ${r.restored}, ${r.failed} could not be restored: ${failed[0]?.error ?? ''}`,
        });
      }
    } else {
      const r = await del.run({ id: p.backup.id });
      if (r) {
        setNote({ ok: true, text: `Backup deleted (${formatBytes(r.freedBytes)} freed).` });
        await load();
      }
    }
  };

  const backups = list.data;
  return (
    <div data-testid="registry-backups" className="flex flex-col gap-3">
      <button
        type="button"
        onClick={onBack}
        data-testid="backups-back"
        className="flex h-10 w-fit items-center gap-1 rounded-xl border border-line bg-surface-2 px-3 text-sm"
      >
        <ArrowLeft size={14} aria-hidden /> Back
      </button>
      <ErrorBanner error={list.error ?? restore.error ?? del.error} />
      {note && (
        <p
          role="status"
          data-testid="backups-note"
          className={`m-0 break-words rounded-xl border p-2 text-xs ${note.ok ? 'border-ok/40 bg-ok/10' : 'border-danger/40 bg-danger/10'}`}
        >
          {note.text}
        </p>
      )}
      {active && (
        <RunProgress
          label={restore.loading ? 'Restoring' : 'Deleting'}
          progress={active.progress}
          onCancel={active.cancel}
          testId="backups-progress"
        />
      )}
      {backups && backups.length === 0 && (
        <div data-testid="backups-empty">
          <EmptyState title="No backups yet" hint="A backup is saved automatically before every fix." />
        </div>
      )}
      {backups && backups.length > 0 && (
        <Card title={`Backups (${backups.length})`} className="!p-0" testId="backups-list">
          <ul className="m-0 list-none divide-y divide-line p-0">
            {backups.map((b) => (
              <li key={b.id} data-testid={`backup-row-${b.id}`} className="flex flex-col gap-2 px-3 py-2">
                <div className="min-w-0">
                  <p className="m-0 break-words text-sm font-medium">{formatWhen(b.createdAt)}</p>
                  <p className="m-0 break-words text-xs text-muted">
                    {plural(b.issueCount, 'item')} - {formatBytes(b.sizeBytes)} - {b.platform}
                  </p>
                </div>
                <div className="flex gap-2">
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => setPending({ kind: 'restore', backup: b })}
                    data-testid={`backup-restore-${b.id}`}
                    className="flex h-10 min-w-0 flex-1 items-center justify-center gap-1 rounded-xl border border-line bg-surface-2 text-sm disabled:opacity-50"
                  >
                    <RotateCcw size={14} aria-hidden /> Restore
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => setPending({ kind: 'delete', backup: b })}
                    data-testid={`backup-delete-${b.id}`}
                    className="flex h-10 min-w-0 flex-1 items-center justify-center gap-1 rounded-xl border border-line bg-surface-2 text-sm text-danger disabled:opacity-50"
                  >
                    <Trash2 size={14} aria-hidden /> Delete
                  </button>
                </div>
              </li>
            ))}
          </ul>
        </Card>
      )}
      <ConfirmSheet
        open={pending !== null}
        title={pending?.kind === 'restore' ? 'Restore this backup?' : 'Delete this backup?'}
        message={
          pending?.kind === 'restore'
            ? 'The saved items are put back as they were. Changes made to them since then are overwritten. You may be asked for administrator rights.'
            : 'The backup is permanently deleted. You will not be able to undo the fix it belongs to.'
        }
        confirmLabel={pending?.kind === 'restore' ? 'Restore' : 'Delete'}
        danger={pending?.kind === 'delete'}
        onConfirm={() => void confirm()}
        onCancel={() => setPending(null)}
      />
    </div>
  );
}
