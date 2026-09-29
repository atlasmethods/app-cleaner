import { useEffect } from 'react';
import { RefreshCw } from 'lucide-react';
import type { SysInfo } from '../api/sysinfo';
import { SYSINFO_GET } from '../api/sysinfo';
import { Card } from '../components/Card';
import { ErrorBanner } from '../components/ErrorBanner';
import { ProgressBar } from '../components/ProgressBar';
import { useCall } from '../hooks/useCall';
import { formatBytes, formatDuration, percent } from '../lib/format';

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex min-w-0 items-baseline justify-between gap-3 py-0.5 text-sm">
      <span className="shrink-0 text-muted">{label}</span>
      <span className="min-w-0 break-words text-right">{value}</span>
    </div>
  );
}

export default function SysInfoPage() {
  const { data, error, loading, run } = useCall<SysInfo>(SYSINFO_GET);

  useEffect(() => {
    void run();
  }, [run]);

  return (
    <div data-testid="page-sysinfo" className="flex flex-col gap-3 p-4">
      <ErrorBanner error={error} onRetry={() => void run()} />

      {!data && loading && (
        <p className="py-8 text-center text-sm text-muted" data-testid="sysinfo-loading">
          Reading system information…
        </p>
      )}

      {data && (
        <>
          <Card title="System" testId="sysinfo-os">
            <Row label="OS" value={`${data.os.name} ${data.os.version}`.trim()} />
            <Row label="Kernel" value={data.os.kernel || '—'} />
            <Row label="Host" value={data.os.hostname || '—'} />
            <Row label="Arch" value={data.os.arch} />
            <Row label="Uptime" value={formatDuration(data.uptimeSecs)} />
          </Card>

          <Card title="CPU" testId="sysinfo-cpu">
            <p className="break-words text-sm font-medium" data-testid="sysinfo-cpu-brand">
              {data.cpu.brand}
            </p>
            <p className="mb-2 text-xs text-muted">{data.cpu.cores} logical cores</p>
            <ProgressBar value={data.cpu.usage} label="CPU usage" tone="usage" />
            <p className="mt-1 text-xs text-muted">{data.cpu.usage.toFixed(0)}% in use</p>
          </Card>

          <Card title="Memory" testId="sysinfo-memory">
            <ProgressBar value={percent(data.memory.used, data.memory.total)} label="Memory usage" tone="usage" />
            <p className="mt-1 text-xs text-muted">
              {formatBytes(data.memory.used)} of {formatBytes(data.memory.total)} used
            </p>
          </Card>

          <Card title="Disks" testId="sysinfo-disks">
            {data.disks.length === 0 && <p className="text-sm text-muted">No disks found.</p>}
            <ul className="m-0 flex list-none flex-col gap-3 p-0">
              {data.disks.map((d) => {
                const used = d.total - d.available;
                return (
                  <li key={`${d.name}:${d.mount}`} className="min-w-0" data-testid="sysinfo-disk">
                    <div className="flex min-w-0 justify-between gap-2 text-sm">
                      <span className="min-w-0 break-all font-medium">{d.mount}</span>
                      <span className="shrink-0 text-xs text-muted">{d.fs}</span>
                    </div>
                    <div className="my-1">
                      <ProgressBar value={percent(used, d.total)} label={`Disk usage ${d.mount}`} tone="usage" />
                    </div>
                    <p className="text-xs text-muted">
                      {formatBytes(d.available)} free of {formatBytes(d.total)}
                    </p>
                  </li>
                );
              })}
            </ul>
          </Card>

          <button
            type="button"
            onClick={() => void run()}
            disabled={loading}
            className="flex h-10 items-center justify-center gap-2 rounded-xl border border-line bg-surface text-sm font-medium disabled:opacity-60"
          >
            <RefreshCw size={16} aria-hidden /> {loading ? 'Refreshing…' : 'Refresh'}
          </button>
        </>
      )}
    </div>
  );
}
