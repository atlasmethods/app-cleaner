import { ExternalLink, History, PlusCircle, RotateCcw, ShieldAlert, Trash2 } from 'lucide-react';
import { useEffect, useState } from 'react';
import type { ListPointsResult, OpResult, RestorePoint } from '../api/restore';
import { Card } from '../components/Card';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { EmptyState } from '../components/EmptyState';
import { ErrorBanner } from '../components/ErrorBanner';
import { RunProgress } from '../components/RunProgress';
import { useCall } from '../hooks/useCall';
import { formatBytes, formatWhen } from '../lib/format';
import {
  backupKindLabel,
  deleteBlockedReason,
  descriptionProblem,
  hasOlderWindowsPoints,
  kindLabel,
  splitPoints,
} from '../lib/restore';

type Pending =
  | { kind: 'create'; description: string }
  | { kind: 'delete'; point: RestorePoint }
  | { kind: 'restore'; point: RestorePoint }
  | { kind: 'delete-old' };

function Badge({ children, tone = 'muted', testId }: { children: string; tone?: 'muted' | 'accent'; testId?: string }) {
  return (
    <span
      data-testid={testId}
      className={`shrink-0 rounded-full px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide ${
        tone === 'accent' ? 'bg-accent/15 text-accent' : 'bg-surface-2 text-muted'
      }`}
    >
      {children}
    </span>
  );
}

export default function SystemRestorePage() {
  const list = useCall<ListPointsResult, { elevate?: boolean }>('restore.list_points');
  const create = useCall<OpResult, { description: string }>('restore.create_point');
  const del = useCall<OpResult, { id: string }>('restore.delete_point');
  const delOld = useCall<OpResult>('restore.delete_old');
  const restore = useCall<OpResult, { id: string }>('restore.restore');
  const openTool = useCall<OpResult>('restore.open_system_tool');

  const [description, setDescription] = useState('');
  const [pending, setPending] = useState<Pending | null>(null);
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
  const [elevated, setElevated] = useState(false);

  const load = list.run;
  useEffect(() => {
    void load();
  }, [load]);

  const ops = [create, del, delOld, restore];
  const active = ops.find((o) => o.loading) ?? null;
  const activeLabel = create.loading
    ? 'Creating restore point'
    : del.loading
      ? 'Deleting'
      : delOld.loading
        ? 'Deleting old restore points'
        : 'Restoring';
  const busy = active !== null || list.loading;
  const data = list.data;
  const { system, backups } = splitPoints(data?.points ?? []);
  const problem = description.trim() === '' ? null : descriptionProblem(description);
  const error = list.error ?? create.error ?? del.error ?? delOld.error ?? restore.error ?? openTool.error;

  const reload = () => load(elevated ? { elevate: true } : undefined);

  const authorize = async () => {
    setElevated(true);
    await load({ elevate: true });
  };

  const finish = async (r: OpResult | undefined, refresh = true) => {
    if (!r) return;
    setNote({ ok: r.ok, text: r.message });
    if (refresh) await reload();
  };

  const confirm = async () => {
    const p = pending;
    setPending(null);
    setNote(null);
    if (!p) return;
    switch (p.kind) {
      case 'create': {
        const r = await create.run({ description: p.description.trim() });
        if (r?.ok) setDescription('');
        return finish(r);
      }
      case 'delete':
        return finish(await del.run({ id: p.point.id }));
      case 'delete-old':
        return finish(await delOld.run());
      case 'restore':
        return finish(await restore.run({ id: p.point.id }), false);
    }
  };

  const confirmCopy = ((): { title: string; message: string; label: string; danger: boolean } => {
    if (!pending) return { title: '', message: '', label: 'Confirm', danger: false };
    switch (pending.kind) {
      case 'create':
        return {
          title: 'Create a restore point?',
          message: `A restore point named "${pending.description.trim()}" is created now. You may be asked for your administrator password. Windows allows one automatic restore point every 24 hours.`,
          label: 'Create',
          danger: false,
        };
      case 'delete':
        return {
          title: 'Delete this restore point?',
          message: `"${pending.point.description || pending.point.id}" is permanently deleted and can no longer be used to roll back.`,
          label: 'Delete',
          danger: true,
        };
      case 'delete-old':
        return {
          title: 'Delete all but the most recent?',
          message: 'Every restore point except the newest one is permanently deleted. You may be asked for administrator rights.',
          label: 'Delete older points',
          danger: true,
        };
      case 'restore':
        return {
          title: 'Restore this backup?',
          message: 'The saved items are put back as they were. Changes made to them since then are overwritten.',
          label: 'Restore',
          danger: false,
        };
    }
  })();

  return (
    <div data-testid="page-restore" className="flex min-h-full flex-col gap-3 p-4">
      <ErrorBanner error={error} onRetry={list.error ? () => void reload() : undefined} />

      {note && (
        <p
          role="status"
          data-testid="restore-note"
          className={`m-0 break-words rounded-xl border p-2 text-xs ${note.ok ? 'border-ok/40 bg-ok/10' : 'border-danger/40 bg-danger/10'}`}
        >
          {note.text}
        </p>
      )}

      {active && (
        <RunProgress label={activeLabel} progress={active.progress} onCancel={active.cancel} testId="restore-progress" />
      )}

      {data && !data.supported && (
        <Card testId="restore-unsupported">
          <div className="flex items-start gap-2">
            <History size={20} className="mt-0.5 shrink-0 text-muted" aria-hidden />
            <div className="min-w-0">
              <p className="m-0 text-sm font-semibold">System restore points are not available</p>
              <p className="m-0 mt-1 break-words text-xs text-muted" data-testid="restore-hint">
                {data.hint}
              </p>
            </div>
          </div>
        </Card>
      )}

      {data?.needsAdmin && (
        <Card testId="restore-needs-admin">
          <div className="flex items-start gap-2">
            <ShieldAlert size={20} className="mt-0.5 shrink-0 text-warn" aria-hidden />
            <div className="min-w-0">
              <p className="m-0 text-sm font-semibold">Administrator rights needed</p>
              <p className="m-0 mt-1 break-words text-xs text-muted">
                Listing the restore points needs administrator rights. You will be asked to approve.
              </p>
            </div>
          </div>
          <button
            type="button"
            onClick={() => void authorize()}
            disabled={busy}
            data-testid="restore-authorize"
            className="mt-2 h-9 w-full rounded-xl border border-line bg-surface-2 text-sm font-medium disabled:opacity-50"
          >
            Authorize and list
          </button>
        </Card>
      )}

      {data?.warnings.map((w) => (
        <p key={w} className="m-0 break-words text-xs text-muted" data-testid="restore-warning">
          {w}
        </p>
      ))}

      {data?.canCreate && (
        <Card title="Create a restore point" testId="restore-create-card">
          <input
            type="text"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="Description, e.g. Before installing X"
            aria-label="Restore point description"
            maxLength={200}
            data-testid="restore-description"
            className="h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 px-3 text-sm"
          />
          {problem && (
            <p className="m-0 mt-1 text-xs text-danger" data-testid="restore-description-problem">
              {problem}
            </p>
          )}
          <button
            type="button"
            onClick={() => setPending({ kind: 'create', description })}
            disabled={busy || description.trim() === '' || problem !== null}
            data-testid="restore-create"
            className="mt-2 flex h-10 w-full items-center justify-center gap-2 rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg disabled:opacity-50"
          >
            <PlusCircle size={16} aria-hidden /> Create restore point
          </button>
        </Card>
      )}

      {data && (data.canOpenSystemTool || data.canDeleteOld) && (
        <div className="flex flex-col gap-2">
          {data.canOpenSystemTool && (
            <button
              type="button"
              onClick={() => void openTool.run().then((r) => r && setNote({ ok: r.ok, text: r.message }))}
              disabled={busy || openTool.loading}
              data-testid="restore-open-tool"
              className="flex h-10 items-center justify-center gap-2 rounded-xl border border-line bg-surface-2 text-sm disabled:opacity-50"
            >
              <ExternalLink size={14} aria-hidden /> Open system restore tool
            </button>
          )}
          {data.canDeleteOld && hasOlderWindowsPoints(system) && (
            <button
              type="button"
              onClick={() => setPending({ kind: 'delete-old' })}
              disabled={busy}
              data-testid="restore-delete-old"
              className="flex h-10 items-center justify-center gap-2 rounded-xl border border-line bg-surface-2 text-sm text-danger disabled:opacity-50"
            >
              <Trash2 size={14} aria-hidden /> Delete all but the most recent
            </button>
          )}
        </div>
      )}

      {data && system.length > 0 && (
        <Card title={`System restore points (${system.length})`} className="!p-0" testId="restore-list">
          <ul className="m-0 list-none divide-y divide-line p-0">
            {system.map((p) => (
              <PointRow key={p.id} point={p} busy={busy} onDelete={() => setPending({ kind: 'delete', point: p })} />
            ))}
          </ul>
        </Card>
      )}

      {data && data.supported && system.length === 0 && !data.needsAdmin && (
        <div data-testid="restore-empty">
          <EmptyState title="No restore points yet" hint="Create one above before making big changes." />
        </div>
      )}

      {data && backups.length > 0 && (
        <Card title={`ClearSweep backups (${backups.length})`} className="!p-0" testId="restore-backups">
          <ul className="m-0 list-none divide-y divide-line p-0">
            {backups.map((p) => (
              <PointRow
                key={p.id}
                point={p}
                busy={busy}
                onDelete={() => setPending({ kind: 'delete', point: p })}
                onRestore={p.restorable ? () => setPending({ kind: 'restore', point: p }) : undefined}
              />
            ))}
          </ul>
        </Card>
      )}

      <ConfirmSheet
        open={pending !== null}
        title={confirmCopy.title}
        message={confirmCopy.message}
        confirmLabel={confirmCopy.label}
        danger={confirmCopy.danger}
        onConfirm={() => void confirm()}
        onCancel={() => setPending(null)}
      />
    </div>
  );
}

function PointRow({
  point,
  busy,
  onDelete,
  onRestore,
}: {
  point: RestorePoint;
  busy: boolean;
  onDelete: () => void;
  onRestore?: () => void;
}) {
  const blocked = deleteBlockedReason(point);
  const isBackup = point.kind === 'clearsweep-backup';
  return (
    <li data-testid={`point-${point.id}`} className="flex flex-col gap-2 px-3 py-2">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-1.5">
          <Badge testId={`point-kind-${point.id}`}>{isBackup ? backupKindLabel(point) : kindLabel(point.kind)}</Badge>
          {point.isNewest && (
            <Badge tone="accent" testId={`point-newest-${point.id}`}>
              Most recent
            </Badge>
          )}
        </div>
        <p className="m-0 mt-1 break-words text-sm font-medium" data-testid="point-description">
          {point.description || point.id}
        </p>
        <p className="m-0 break-words text-xs text-muted" data-testid="point-meta">
          {[
            point.createdAt ? formatWhen(point.createdAt) : null,
            point.sizeBytes !== undefined ? formatBytes(point.sizeBytes) : null,
            point.note && !point.isNewest ? point.note : null,
          ]
            .filter(Boolean)
            .join(' - ')}
        </p>
      </div>
      <div className="flex gap-2">
        {onRestore && (
          <button
            type="button"
            onClick={onRestore}
            disabled={busy}
            data-testid={`point-restore-${point.id}`}
            className="flex h-9 min-w-0 flex-1 items-center justify-center gap-1 rounded-xl border border-line bg-surface-2 text-sm disabled:opacity-50"
          >
            <RotateCcw size={14} aria-hidden /> Restore
          </button>
        )}
        <button
          type="button"
          onClick={onDelete}
          disabled={busy || blocked !== null}
          title={blocked ?? undefined}
          aria-label={blocked ? `Delete (unavailable: ${blocked})` : 'Delete'}
          data-testid={`point-delete-${point.id}`}
          className="flex h-9 min-w-0 flex-1 items-center justify-center gap-1 rounded-xl border border-line bg-surface-2 text-sm text-danger disabled:opacity-50"
        >
          <Trash2 size={14} aria-hidden /> Delete
        </button>
      </div>
      {blocked && (
        <p className="m-0 break-words text-[11px] text-muted" data-testid={`point-blocked-${point.id}`}>
          {blocked}
        </p>
      )}
    </li>
  );
}
