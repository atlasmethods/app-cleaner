import { Eraser, HardDrive, Loader2, ShieldAlert, Usb } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import type {
  DeviceListing,
  DriveWipeReport,
  FreeSpaceReport,
  Passes,
  WiperDevice,
  WiperDrive,
} from '../api/wiper';
import { Card } from '../components/Card';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { ErrorBanner } from '../components/ErrorBanner';
import { RunProgress } from '../components/RunProgress';
import { useCall } from '../hooks/useCall';
import { formatBytes, formatDuration } from '../lib/format';
import { PASS_OPTIONS, canWipeDrive, deviceBlockReason, isPasses } from '../lib/wipe';

type Mode = 'free' | 'drive';

function Badge({ children, tone, testId }: { children: string; tone: 'danger' | 'muted'; testId?: string }) {
  return (
    <span
      data-testid={testId}
      className={`shrink-0 rounded-full px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide ${
        tone === 'danger' ? 'bg-danger/10 text-danger' : 'bg-surface-2 text-muted'
      }`}
    >
      {children}
    </span>
  );
}

export default function DriveWiperPage() {
  const drivesCall = useCall<WiperDrive[]>('wiper.list_drives');
  const devicesCall = useCall<DeviceListing>('wiper.list_devices');
  const freeCall = useCall<FreeSpaceReport, { mount: string; passes: Passes }>('wiper.wipe_free_space');
  const driveCall = useCall<DriveWipeReport, { device: string; passes: Passes; confirm: string }>('wiper.wipe_drive');

  const [mode, setMode] = useState<Mode>('free');
  const [passes, setPasses] = useState<Passes>(1);
  const [mount, setMount] = useState<string | null>(null);
  const [device, setDevice] = useState<string | null>(null);
  const [typed, setTyped] = useState('');
  const [confirm, setConfirm] = useState(false);
  const [done, setDone] = useState<{ kind: Mode; text: string } | null>(null);

  const runDrives = drivesCall.run;
  const runDevices = devicesCall.run;
  useEffect(() => {
    void runDrives();
    void runDevices();
  }, [runDrives, runDevices]);

  const drives = drivesCall.data ?? [];
  const listing = devicesCall.data;
  const devices = listing?.devices ?? [];
  const chosenDevice: WiperDevice | null = devices.find((d) => d.device === device) ?? null;
  const chosenDrive = drives.find((d) => d.mount === mount) ?? null;
  const running = freeCall.loading || driveCall.loading;

  const ready = useMemo(
    () => (mode === 'free' ? chosenDrive !== null : canWipeDrive(chosenDevice, typed)),
    [mode, chosenDrive, chosenDevice, typed],
  );

  const switchMode = (m: Mode) => {
    setMode(m);
    setDone(null);
    setTyped('');
  };

  const start = async () => {
    setConfirm(false);
    setDone(null);
    if (mode === 'free' && chosenDrive) {
      const r = await freeCall.run({ mount: chosenDrive.mount, passes });
      if (r) {
        setDone({
          kind: 'free',
          text: `Wiped ${formatBytes(r.bytesPerPass)} of free space on ${r.mount} with ${r.passes} ${r.passes === 1 ? 'pass' : 'passes'} in ${formatDuration(r.durationMs / 1000)}. Temporary files were removed.`,
        });
        void runDrives();
      }
    } else if (mode === 'drive' && chosenDevice) {
      const r = await driveCall.run({ device: chosenDevice.device, passes, confirm: typed });
      if (r) {
        setDone({
          kind: 'drive',
          text: `${r.device} was overwritten with ${r.passes} ${r.passes === 1 ? 'pass' : 'passes'}. Its partitions and files are gone.`,
        });
        setTyped('');
        void runDevices();
      }
    }
  };

  const error = freeCall.error ?? driveCall.error ?? drivesCall.error ?? devicesCall.error;
  const target = mode === 'free' ? chosenDrive?.mount : chosenDevice?.device;

  return (
    <div data-testid="page-wiper" className="flex min-h-full flex-col gap-3 p-4">
      <ErrorBanner error={error} />

      <div
        role="note"
        data-testid="wiper-warning"
        className="flex min-w-0 items-start gap-2 rounded-xl border border-danger/40 bg-danger/10 p-3 text-sm"
      >
        <ShieldAlert size={18} className="mt-0.5 shrink-0 text-danger" aria-hidden />
        <div className="min-w-0">
          {mode === 'free' ? (
            <>
              <p className="m-0 font-semibold">Free-space wiping fills the drive completely for a while.</p>
              <p className="m-0 mt-1 text-xs">
                Your files are not touched, but other programs cannot save anything until it finishes. Close what you are
                working on first. On SSDs and copy-on-write filesystems this is best effort.
              </p>
            </>
          ) : (
            <>
              <p className="m-0 font-semibold">Wiping an entire drive destroys EVERYTHING on it.</p>
              <p className="m-0 mt-1 text-xs">
                All partitions and files are lost for good; there is no undo. Double-check the device name. Drives in use
                and the system drive cannot be wiped.
              </p>
            </>
          )}
        </div>
      </div>

      <div role="radiogroup" aria-label="What to wipe" className="flex gap-1 rounded-xl bg-surface-2 p-1">
        {(
          [
            ['free', 'Free space only'],
            ['drive', 'Entire drive'],
          ] as const
        ).map(([m, label]) => (
          <button
            key={m}
            type="button"
            role="radio"
            aria-checked={mode === m}
            disabled={running}
            onClick={() => switchMode(m)}
            data-testid={`wiper-mode-${m}`}
            className={`h-10 min-w-0 flex-1 rounded-lg border-0 text-sm font-medium ${
              mode === m ? 'bg-surface shadow-sm' : 'bg-transparent text-muted'
            }`}
          >
            {label}
          </button>
        ))}
      </div>

      {mode === 'free' && (
        <Card title="Drive" testId="wiper-drives">
          {drivesCall.loading && drives.length === 0 && <p className="m-0 text-sm text-muted">Looking for drives...</p>}
          <ul className="m-0 flex list-none flex-col gap-2 p-0">
            {drives.map((d) => {
              const on = mount === d.mount;
              return (
                <li key={d.mount}>
                  <button
                    type="button"
                    role="radio"
                    aria-checked={on}
                    disabled={running}
                    onClick={() => {
                      setMount(d.mount);
                      setDone(null);
                    }}
                    data-testid={`wiper-drive-${d.mount}`}
                    className={`flex w-full min-w-0 items-center gap-2 rounded-xl border px-3 py-2 text-left ${
                      on ? 'border-accent bg-accent/10' : 'border-line bg-surface-2'
                    }`}
                  >
                    {d.removable ? <Usb size={18} className="shrink-0" aria-hidden /> : <HardDrive size={18} className="shrink-0" aria-hidden />}
                    <span className="min-w-0 flex-1">
                      <span className="block break-all text-sm font-medium">{d.mount}</span>
                      <span className="block text-[11px] text-muted">
                        {d.fs} - {formatBytes(d.available)} free of {formatBytes(d.total)}
                      </span>
                    </span>
                    {d.isSystem && <Badge tone="danger" testId="wiper-system-badge">System</Badge>}
                    {d.removable && <Badge tone="muted">Removable</Badge>}
                  </button>
                </li>
              );
            })}
          </ul>
          {chosenDrive?.isSystem && (
            <p className="m-0 mt-2 text-xs text-warn" data-testid="wiper-system-note">
              This is the system drive. Wiping its free space is allowed, but the computer may feel stuck until it is done.
            </p>
          )}
        </Card>
      )}

      {mode === 'drive' && (
        <Card title="Physical drive" testId="wiper-devices">
          {listing && !listing.supported && (
            <p className="m-0 text-sm text-muted" data-testid="wiper-unsupported">
              Wiping a whole drive is not available on this operating system yet. Use free-space wiping instead.
            </p>
          )}
          {listing?.supported && devices.length === 0 && (
            <p className="m-0 text-sm text-muted" data-testid="wiper-no-devices">
              No physical drives were found.
            </p>
          )}
          <ul className="m-0 flex list-none flex-col gap-2 p-0">
            {devices.map((d) => {
              const reason = deviceBlockReason(d);
              const on = device === d.device;
              return (
                <li key={d.device}>
                  <button
                    type="button"
                    role="radio"
                    aria-checked={on}
                    aria-disabled={reason !== null}
                    disabled={reason !== null || running}
                    onClick={() => {
                      setDevice(d.device);
                      setTyped('');
                      setDone(null);
                    }}
                    data-testid={`wiper-device-${d.device}`}
                    className={`flex w-full min-w-0 items-center gap-2 rounded-xl border px-3 py-2 text-left disabled:opacity-60 ${
                      on ? 'border-danger bg-danger/10' : 'border-line bg-surface-2'
                    }`}
                  >
                    {d.removable ? <Usb size={18} className="shrink-0" aria-hidden /> : <HardDrive size={18} className="shrink-0" aria-hidden />}
                    <span className="min-w-0 flex-1">
                      <span className="block break-all text-sm font-medium">{d.device}</span>
                      <span className="block break-words text-[11px] text-muted">
                        {[d.model, formatBytes(d.sizeBytes), `${d.partitions.length} ${d.partitions.length === 1 ? 'partition' : 'partitions'}`]
                          .filter(Boolean)
                          .join(' - ')}
                      </span>
                      {reason && (
                        <span className="block break-words text-[11px] text-warn" data-testid={`wiper-reason-${d.device}`}>
                          {reason}
                        </span>
                      )}
                    </span>
                    {d.isSystem && <Badge tone="danger" testId="wiper-system-badge">System</Badge>}
                  </button>
                </li>
              );
            })}
          </ul>
        </Card>
      )}

      <Card title="Overwrite passes">
        <label className="sr-only" htmlFor="wiper-passes">
          Overwrite passes
        </label>
        <select
          id="wiper-passes"
          value={passes}
          disabled={running}
          onChange={(e) => {
            const n = Number(e.target.value);
            if (isPasses(n)) setPasses(n);
          }}
          data-testid="wiper-passes"
          className="h-10 w-full rounded-xl border border-line bg-surface-2 px-3 text-sm"
        >
          {PASS_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label} - {o.hint}
            </option>
          ))}
        </select>
        <p className="m-0 mt-2 text-[11px] text-muted">More passes take proportionally longer. One pass is enough for modern drives.</p>
      </Card>

      {mode === 'drive' && chosenDevice && (
        <Card title="Confirm">
          <label className="block text-sm" htmlFor="wiper-confirm-input">
            Type <span className="break-all font-mono font-semibold" data-testid="wiper-confirm-target">{chosenDevice.device}</span> to confirm
          </label>
          <input
            id="wiper-confirm-input"
            type="text"
            value={typed}
            disabled={running}
            onChange={(e) => setTyped(e.target.value)}
            autoComplete="off"
            autoCapitalize="off"
            spellCheck={false}
            data-testid="wiper-confirm-input"
            className="mt-2 h-10 w-full min-w-0 rounded-xl border border-line bg-surface-2 px-3 font-mono text-sm"
          />
        </Card>
      )}

      {running && (
        <RunProgress
          label={mode === 'free' ? 'Wiping free space' : 'Wiping drive'}
          progress={mode === 'free' ? freeCall.progress : driveCall.progress}
          onCancel={mode === 'free' ? freeCall.cancel : driveCall.cancel}
          testId="wiper-progress"
          note={
            mode === 'drive'
              ? 'Cancelling leaves the drive partly overwritten and unusable until it is formatted again.'
              : 'Cancelling removes the temporary files and gives the space back.'
          }
        />
      )}

      {done && !running && (
        <p role="status" className="m-0 break-words rounded-xl border border-ok/40 bg-ok/10 p-3 text-sm" data-testid="wiper-result">
          {done.text}
        </p>
      )}

      <div className="sticky bottom-0 z-10 mt-auto rounded-xl border border-line bg-surface p-2 shadow-lg">
        <button
          type="button"
          onClick={() => setConfirm(true)}
          disabled={!ready || running}
          data-testid="btn-wiper-start"
          className="flex h-10 w-full items-center justify-center gap-1.5 rounded-xl border-0 bg-danger text-sm font-semibold text-danger-fg disabled:opacity-50"
        >
          {running ? <Loader2 size={16} className="animate-spin" aria-hidden /> : <Eraser size={16} aria-hidden />}
          {mode === 'free' ? 'Wipe free space' : 'Wipe entire drive'}
        </button>
      </div>

      <ConfirmSheet
        open={confirm}
        title={mode === 'free' ? `Wipe free space on ${target ?? ''}?` : `Erase everything on ${target ?? ''}?`}
        message={
          mode === 'free'
            ? `The free space will be overwritten ${passes} ${passes === 1 ? 'time' : 'times'}. Your files stay. This can take a long time.`
            : `Every file and partition on ${target ?? ''} will be destroyed permanently with ${passes} ${passes === 1 ? 'pass' : 'passes'}. This cannot be undone.`
        }
        confirmLabel={mode === 'free' ? 'Wipe free space' : 'Erase drive'}
        danger
        onConfirm={() => void start()}
        onCancel={() => setConfirm(false)}
      />
    </div>
  );
}
