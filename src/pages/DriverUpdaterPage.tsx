import { useMemo, useState } from 'react';
import { CheckCircle2, HardDriveDownload, Loader2, RotateCw, ScanSearch, XCircle } from 'lucide-react';
import type { BackupResult, DriverEntry, DriverUpdateReport } from '../api/driver_updater';
import { Card } from '../components/Card';
import { Checkbox } from '../components/Checkbox';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { EmptyState } from '../components/EmptyState';
import { ErrorBanner } from '../components/ErrorBanner';
import { ProgressBar } from '../components/ProgressBar';
import { useCall } from '../hooks/useCall';
import { isWindowsClient } from '../lib/platform';
import { driverSourceLabel, driverVersions } from '../lib/drivers';
import { toggleId } from '../lib/updates';

type Confirm = 'update' | 'backup' | null;

export default function DriverUpdaterPage() {
  const scan = useCall<DriverEntry[]>('driver_updater.scan');
  const update = useCall<DriverUpdateReport, { ids: string[] }>('driver_updater.update');
  const backup = useCall<BackupResult>('driver_updater.backup');
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [confirm, setConfirm] = useState<Confirm>(null);
  const [report, setReport] = useState<DriverUpdateReport | null>(null);
  const [backupNote, setBackupNote] = useState<{ ok: boolean; text: string } | null>(null);
  const windows = isWindowsClient();

  const drivers = scan.data;
  const ids = useMemo(() => (drivers ?? []).map((d) => d.id).filter((id) => chosen.has(id)), [drivers, chosen]);
  const busy = scan.loading || update.loading || backup.loading;
  const active = scan.loading ? scan : update.loading ? update : backup.loading ? backup : null;
  const results = useMemo(() => new Map((report?.results ?? []).map((r) => [r.id, r])), [report]);
  const needsReboot = report?.rebootRequired ?? false;
  const error = scan.error ?? update.error ?? backup.error;

  const runScan = async () => {
    setReport(null);
    setBackupNote(null);
    setChosen(new Set());
    await scan.run();
  };

  const runUpdate = async () => {
    setConfirm(null);
    setReport(null);
    const r = await update.run({ ids });
    if (!r) return;
    setReport(r);
    setChosen(new Set());
    await scan.run();
  };

  const runBackup = async () => {
    setConfirm(null);
    setBackupNote(null);
    const r = await backup.run();
    if (r) setBackupNote({ ok: r.ok, text: r.message });
  };

  return (
    <div data-testid="page-drivers" className="flex min-h-full flex-col gap-3 p-4">
      <ErrorBanner error={error} />

      <div className="flex gap-2">
        <button
          type="button"
          onClick={() => void runScan()}
          disabled={busy}
          data-testid="btn-scan"
          className="flex h-10 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border-0 bg-accent px-3 text-sm font-semibold text-accent-fg disabled:opacity-50"
        >
          {scan.loading ? <Loader2 size={16} className="animate-spin" aria-hidden /> : <ScanSearch size={16} aria-hidden />}
          {drivers === null ? 'Scan' : 'Scan again'}
        </button>
        {windows && (
          <button
            type="button"
            onClick={() => setConfirm('backup')}
            disabled={busy}
            data-testid="btn-backup"
            className="flex h-10 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border border-line bg-surface-2 px-2 text-sm font-medium disabled:opacity-50"
          >
            <HardDriveDownload size={16} aria-hidden /> Back up drivers
          </button>
        )}
      </div>

      {backupNote && (
        <p
          className={`m-0 break-words rounded-xl border p-2 text-xs ${backupNote.ok ? 'border-ok/40 bg-ok/10' : 'border-danger/40 bg-danger/10'}`}
          data-testid="backup-note"
          role="status"
        >
          {backupNote.text}
        </p>
      )}

      {active && (
        <Card testId="drivers-progress">
          <p className="m-0 mb-2 break-words text-xs text-muted" data-testid="drivers-progress-message">
            {active.progress?.message ?? (active === scan ? 'Scanning for driver updates...' : 'Working...')}
          </p>
          <ProgressBar value={(active.progress?.fraction ?? 0) * 100} label="Progress" />
          <button
            type="button"
            onClick={() => active.cancel()}
            data-testid="btn-cancel"
            className="mt-2 h-9 w-full rounded-xl border border-line bg-surface-2 text-sm font-medium"
          >
            Cancel
          </button>
        </Card>
      )}

      {needsReboot && !busy && (
        <p
          className="m-0 flex items-start gap-2 rounded-xl border border-warn/40 bg-warn/10 p-2 text-xs"
          data-testid="reboot-notice"
          role="status"
        >
          <RotateCw size={14} className="mt-0.5 shrink-0" aria-hidden /> Restart your computer to finish installing the updates.
        </p>
      )}

      {report && !busy && (
        <Card
          title={`Update result: ${report.succeeded} succeeded${report.failed > 0 ? `, ${report.failed} failed` : ''}`}
          testId="drivers-report"
          className="!p-0"
        >
          <ul className="m-0 list-none divide-y divide-line p-0">
            {report.results.map((r) => (
              <li key={r.id} className="flex min-w-0 items-start gap-2 px-3 py-1.5" data-testid={`driver-report-${r.id}`}>
                {r.ok ? (
                  <CheckCircle2 size={16} className="mt-0.5 shrink-0 text-ok" aria-label="Updated" />
                ) : (
                  <XCircle size={16} className="mt-0.5 shrink-0 text-danger" aria-label="Failed" />
                )}
                <span className="min-w-0 flex-1">
                  <span className="block break-words text-sm">{r.deviceName || r.id}</span>
                  <span className="block break-words text-xs text-muted">{r.message}</span>
                </span>
              </li>
            ))}
          </ul>
          {report.backupPath && (
            <p className="m-0 break-all border-t border-line px-3 py-1.5 text-[11px] text-muted" data-testid="drivers-backup-path">
              Backup: {report.backupPath}
            </p>
          )}
        </Card>
      )}

      {drivers === null && !scan.loading && !scan.error && (
        <EmptyState
          icon={ScanSearch}
          title="Scan for driver updates"
          hint="Checks firmware and driver updates from your system's own update sources."
        />
      )}

      {drivers !== null && drivers.length === 0 && !busy && (
        <div data-testid="drivers-empty">
          <EmptyState icon={CheckCircle2} title="No driver updates found" hint="Your drivers and firmware are up to date." />
        </div>
      )}

      {drivers !== null && drivers.length > 0 && (
        <Card title={`Available updates (${drivers.length})`} testId="drivers-list" className="!p-0">
          <ul className="m-0 list-none divide-y divide-line p-0">
            {drivers.map((d) => {
              const res = results.get(d.id);
              return (
                <li key={d.id} className="flex min-w-0 items-start gap-2 px-3 py-2" data-testid={`driver-${d.id}`}>
                  <span className="mt-0.5">
                    <Checkbox
                      checked={chosen.has(d.id)}
                      disabled={busy}
                      onChange={() => setChosen((s) => toggleId(s, d.id))}
                      ariaLabel={`Select ${d.deviceName}`}
                      testId={`driver-select-${d.id}`}
                    />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5">
                      <span className="break-words text-sm font-medium" data-testid="driver-name">
                        {d.deviceName}
                      </span>
                      <span className="rounded bg-surface-2 px-1 text-[11px] text-muted">{driverSourceLabel(d.source)}</span>
                      {d.rebootRequired && (
                        <span className="rounded bg-warn/15 px-1 text-[11px] text-warn" data-testid="driver-reboot">
                          Restart needed
                        </span>
                      )}
                    </span>
                    {(d.vendor || driverVersions(d)) && (
                      <span className="block break-words text-xs text-muted" data-testid="driver-versions">
                        {[d.vendor, driverVersions(d)].filter(Boolean).join(' - ')}
                      </span>
                    )}
                    <span className="block break-words text-xs text-muted">{d.description}</span>
                    {res && (
                      <span className={`block break-words text-xs ${res.ok ? 'text-ok' : 'text-danger'}`} data-testid={`driver-result-${d.id}`}>
                        {res.ok ? 'Updated' : `Failed: ${res.message}`}
                      </span>
                    )}
                  </span>
                </li>
              );
            })}
          </ul>
        </Card>
      )}

      <div
        data-testid="action-bar"
        className="sticky bottom-0 z-10 mt-auto flex gap-2 rounded-xl border border-line bg-surface p-2 shadow-lg"
      >
        <button
          type="button"
          onClick={() => setConfirm('update')}
          disabled={busy || ids.length === 0}
          data-testid="btn-update-selected"
          className="flex h-10 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border-0 bg-accent px-2 text-sm font-semibold text-accent-fg disabled:opacity-50"
        >
          {update.loading && <Loader2 size={16} className="animate-spin" aria-hidden />}
          Update selected ({ids.length})
        </button>
      </div>

      <ConfirmSheet
        open={confirm === 'update'}
        title={`Install ${ids.length} driver ${ids.length === 1 ? 'update' : 'updates'}?`}
        message={`${
          windows ? 'Your installed drivers are backed up first. ' : ''
        }Firmware and driver updates can require a restart, and a failed firmware update can make a device unusable. Keep your computer plugged in. You may be asked for your administrator password.`}
        confirmLabel="Install"
        danger
        onConfirm={() => void runUpdate()}
        onCancel={() => setConfirm(null)}
      />
      <ConfirmSheet
        open={confirm === 'backup'}
        title="Back up installed drivers?"
        message="Exports every third-party driver in the Windows driver store to a folder in ClearSweep's data directory. You may be asked for permission."
        confirmLabel="Back up"
        onConfirm={() => void runBackup()}
        onCancel={() => setConfirm(null)}
      />
    </div>
  );
}
