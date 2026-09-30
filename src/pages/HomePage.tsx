import { useCallback, useEffect, useMemo, useState } from 'react';
import { Gauge, Loader2, RefreshCw, ScanSearch, Sparkles, Wrench } from 'lucide-react';
import type { CategoryId, FixParams, FixReport, HealthReport } from '../api/health';
import { Card } from '../components/Card';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { ErrorBanner } from '../components/ErrorBanner';
import { RunProgress } from '../components/RunProgress';
import { Tile } from '../components/Tile';
import { useCall } from '../hooks/useCall';
import { useSettings } from '../hooks/useSettings';
import { formatWhen } from '../lib/format';
import {
  CATEGORY_ORDER,
  blockedBrowsers,
  canFix,
  categoryOf,
  confirmMessage,
  defaultSelection,
  isEmptyFix,
  mergeFixReports,
  retryParams,
  statusLine,
  toFixParams,
  type Selection,
} from '../lib/health';
import { CategoryCard } from './home/CategoryCard';
import { FindingsSheet } from './home/FindingsSheet';
import { FixResultCard } from './home/FixResultCard';
import { ScoreRing } from './home/ScoreRing';

const NOT_FINISHED =
  'The fix did not finish. Some changes may already have been made, so run a scan to see where things stand.';

/**
 * Health Check: scan four areas (privacy, space, speed, security), review what was found and
 * fix it in one go. What "Fix all" does is exactly what the user sees ticked in the sheets.
 */
export default function HomePage() {
  const last = useCall<HealthReport | null>('health.last');
  const scan = useCall<HealthReport>('health.analyze');
  const fix = useCall<FixReport, FixParams>('health.fix');
  const { settings } = useSettings();

  // The result of a scan (or of the re-scan after a fix) made in this session. Only this one
  // can be fixed; `health.last` merely shows the previous score at once.
  const [fresh, setFresh] = useState<HealthReport | null>(null);
  // `null` = untouched by the user: follow the defaults for the report shown.
  const [selection, setSelection] = useState<Selection | null>(null);
  const [openCat, setOpenCat] = useState<CategoryId | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [result, setResult] = useState<FixReport | null>(null);
  const [scoreBefore, setScoreBefore] = useState<number | null>(null);
  const [askClose, setAskClose] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const runLast = last.run;
  useEffect(() => {
    void runLast();
  }, [runLast]);

  const shown: HealthReport | null = fresh ?? last.data ?? null;
  const stale = fresh === null && shown !== null;
  const sel = useMemo(() => selection ?? (shown ? defaultSelection(shown) : null), [selection, shown]);
  const params = useMemo(() => (fresh && sel ? toFixParams(fresh, sel) : {}), [fresh, sel]);
  const busy = scan.loading || fix.loading;
  const anythingFixable = fresh ? CATEGORY_ORDER.some((id) => canFix(categoryOf(fresh, id))) : false;

  // ------------------------------------------------------------ scan

  const startScan = async () => {
    setNotice(null);
    setResult(null);
    setAskClose(false);
    const r = await scan.run();
    if (r) {
      setFresh(r);
      setSelection(null);
    }
  };

  // ------------------------------------------------------------ fix

  /** What the machine looks like is unknown after an interrupted fix: forget both reports. */
  const forgetReports = useCallback(() => {
    setFresh(null);
    setSelection(null);
    void runLast();
  }, [runLast]);

  const finishFix = (r: FixReport | undefined, previous: FixReport | null) => {
    if (!r) {
      forgetReports();
      setNotice(NOT_FINISHED);
      return;
    }
    const merged = previous ? mergeFixReports(previous, r) : r;
    setResult(merged);
    if (r.report) {
      setFresh(r.report);
      setSelection(null);
    } else {
      forgetReports();
    }
    if (r.cancelled) setNotice(NOT_FINISHED);
    // Only an "ask" policy lets the user decide now; "skip" and "always" were applied by the server.
    if (!previous && settings?.closeBrowsers === 'ask' && retryParams(merged)) setAskClose(true);
  };

  const doFix = async () => {
    if (!fresh || isEmptyFix(params)) return;
    setConfirming(false);
    setNotice(null);
    setResult(null);
    setAskClose(false);
    setScoreBefore(fresh.score);
    const r = await fix.run({ ...params, closeApps: settings?.closeBrowsers === 'always' ? 'always' : 'skip' });
    finishFix(r, null);
  };

  const closeAndFix = async () => {
    const p = result ? retryParams(result) : null;
    setAskClose(false);
    if (!p || !result) return;
    const prev = result;
    const r = await fix.run(p);
    finishFix(r, prev);
  };

  const errors = [last.error, scan.error, fix.error];
  const firstError = errors.find((e) => e !== null) ?? null;
  const progress = scan.loading ? scan.progress : fix.loading ? fix.progress : null;
  const blocked = result ? blockedBrowsers(result) : [];

  return (
    <div data-testid="page-home" className="flex min-h-full min-w-0 flex-col gap-3 p-4">
      <ErrorBanner error={firstError} onRetry={last.error ? () => void runLast() : undefined} />

      <Card testId="health-hero" className="flex flex-col items-center gap-2 text-center">
        <ScoreRing score={shown?.score ?? null} stale={stale} />
        <p className="m-0 break-words text-sm font-semibold" data-testid="health-status" aria-live="polite">
          {shown ? statusLine(shown) : 'Not scanned yet'}
        </p>
        <p className="m-0 break-words text-xs text-muted" data-testid="health-scanned">
          {shown
            ? `${stale ? 'Last scan' : 'Scanned'} ${formatWhen(shown.scannedAt)}`
            : 'Check privacy, junk, speed and security in one go.'}
        </p>
        <button
          type="button"
          onClick={() => void startScan()}
          disabled={busy}
          data-testid="btn-scan"
          className="flex h-10 w-full items-center justify-center gap-1.5 rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg disabled:opacity-50"
        >
          {scan.loading ? (
            <Loader2 size={16} className="animate-spin" aria-hidden />
          ) : shown ? (
            <RefreshCw size={16} aria-hidden />
          ) : (
            <ScanSearch size={16} aria-hidden />
          )}
          {shown ? 'Rescan' : 'Scan'}
        </button>
      </Card>

      {busy && (
        <RunProgress
          testId="health-progress"
          label={scan.loading ? 'Scanning' : 'Fixing'}
          progress={progress}
          onCancel={() => (scan.loading ? scan.cancel() : fix.cancel())}
          note={fix.loading ? 'Cancelling stops after the current step; what is already done stays done.' : undefined}
        />
      )}

      {notice && !busy && (
        <p className="m-0 break-words rounded-xl border border-warn/40 bg-warn/10 p-3 text-xs" data-testid="health-notice" role="status">
          {notice}
        </p>
      )}

      {result && !busy && (
        <FixResultCard
          result={result}
          before={scoreBefore}
          after={fresh?.score ?? null}
          onCloseAndFix={() => void closeAndFix()}
          busy={busy}
        />
      )}

      {shown && (
        <section className="flex flex-col gap-2" aria-label="Health categories" data-testid="health-categories">
          {stale && (
            <p className="m-0 text-xs text-muted" data-testid="health-stale">
              From your last scan. Rescan to check again and fix what is found.
            </p>
          )}
          {CATEGORY_ORDER.map((id) => {
            const c = categoryOf(shown, id);
            return c ? <CategoryCard key={id} category={c} stale={stale} onOpen={() => setOpenCat(id)} /> : null;
          })}
        </section>
      )}

      {fresh && !busy && (
        <div
          data-testid="action-bar"
          className="sticky bottom-0 z-10 flex gap-2 rounded-xl border border-line bg-surface p-2 shadow-lg"
        >
          <button
            type="button"
            onClick={() => setConfirming(true)}
            disabled={!anythingFixable || isEmptyFix(params)}
            data-testid="btn-fix-all"
            className="flex h-10 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg disabled:opacity-50"
          >
            <Wrench size={16} aria-hidden />
            {anythingFixable ? 'Fix all' : 'Nothing to fix'}
          </button>
        </div>
      )}

      <nav aria-label="Quick links" className="grid grid-cols-3 gap-2" data-testid="quick-links">
        <Tile to="/clean" label="Clean" icon={Sparkles} testId="link-clean" />
        <Tile to="/performance" label="Performance" icon={Gauge} testId="link-performance" />
        <Tile to="/tools/updater" label="Software Updater" icon={RefreshCw} testId="link-updater" />
      </nav>

      {shown && sel && (
        <FindingsSheet
          category={openCat ? (categoryOf(shown, openCat) ?? null) : null}
          report={shown}
          selection={sel}
          onChange={setSelection}
          onClose={() => setOpenCat(null)}
          readOnly={stale}
        />
      )}

      <ConfirmSheet
        open={confirming && fresh !== null}
        title="Fix selected issues?"
        message={fresh ? confirmMessage(fresh, params) : undefined}
        confirmLabel="Fix"
        danger
        onConfirm={() => void doFix()}
        onCancel={() => setConfirming(false)}
      />

      <ConfirmSheet
        open={askClose}
        title="Close running browsers?"
        message={`${blocked.join(', ')} ${blocked.length === 1 ? 'is' : 'are'} running, so ${
          blocked.length === 1 ? 'its' : 'their'
        } data was not cleaned. Close ${blocked.length === 1 ? 'it' : 'them'} now and fix? Save any open work first.`}
        confirmLabel="Close browsers & fix"
        cancelLabel="Skip"
        danger
        onConfirm={() => void closeAndFix()}
        onCancel={() => setAskClose(false)}
      />
    </div>
  );
}
