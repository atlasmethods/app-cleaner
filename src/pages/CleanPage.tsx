import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Loader2, Sparkles, Trash2 } from 'lucide-react';
import type {
  AnalyzeReport,
  CleanParams,
  CleanReport,
  GroupInfo,
  RuleInfo,
  RulesListing,
} from '../api/cleaner';
import { Card } from '../components/Card';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { ErrorBanner } from '../components/ErrorBanner';
import { ProgressBar } from '../components/ProgressBar';
import { useCall } from '../hooks/useCall';
import { useSettings } from '../hooks/useSettings';
import {
  applyPlan,
  hasContent,
  orderedSelection,
  planGroupToggle,
  planRuleToggle,
  selectionOf,
  visibleResults,
  type TogglePlan,
} from '../lib/cleanSelection';
import { formatBytes } from '../lib/format';
import { blockedApps, blockedRuleIds, mergeReports } from '../lib/mergeReports';
import { CleanSummary } from './clean/CleanSummary';
import { ResultsList } from './clean/ResultsList';
import { RuleTree } from './clean/RuleTree';

function toggled(set: ReadonlySet<string>, key: string): Set<string> {
  const next = new Set(set);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

function sameSet(a: ReadonlySet<string>, b: ReadonlySet<string>): boolean {
  return a.size === b.size && [...a].every((x) => b.has(x));
}

export default function CleanPage() {
  const rules = useCall<RulesListing>('cleaner.list_rules');
  const analyze = useCall<AnalyzeReport, { ruleIds: string[] }>('cleaner.analyze');
  const clean = useCall<CleanReport, CleanParams>('cleaner.clean');
  const { settings, patch, error: settingsError, clearError } = useSettings();

  // `null` = untouched by the user: follow whatever the server says is enabled.
  const [chosen, setChosen] = useState<Set<string> | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [openResults, setOpenResults] = useState<Set<string>>(new Set());
  const [analyzedFor, setAnalyzedFor] = useState<Set<string> | null>(null);
  const [warnPlan, setWarnPlan] = useState<TogglePlan | null>(null);
  const [confirmClean, setConfirmClean] = useState(false);
  const [blocked, setBlocked] = useState<string[] | null>(null);
  const [summary, setSummary] = useState<CleanReport | null>(null);

  const listing = rules.data;
  const selected = useMemo(
    () => chosen ?? (listing ? selectionOf(listing) : new Set<string>()),
    [chosen, listing],
  );
  const names = useMemo(() => {
    const m: Record<string, string> = {};
    if (listing) {
      for (const c of listing.categories)
        for (const g of c.groups) for (const r of g.rules) m[r.id] = `${g.group} - ${r.name}`;
    }
    return m;
  }, [listing]);

  const runRules = rules.run;
  useEffect(() => {
    void runRules();
  }, [runRules]);

  // Selection changes are saved one at a time, in click order.
  const saveChain = useRef<Promise<unknown>>(Promise.resolve());
  const persist = useCallback(
    (next: Set<string>) => {
      if (!listing) return;
      const ids = orderedSelection(listing, next);
      saveChain.current = saveChain.current.then(() => patch({ selectedRules: ids }));
    },
    [listing, patch],
  );

  const apply = useCallback(
    (plan: TogglePlan) => {
      const next = applyPlan(selected, plan);
      setChosen(next);
      persist(next);
      setSummary(null);
    },
    [selected, persist],
  );

  const requestPlan = useCallback(
    (plan: TogglePlan) => {
      if (plan.warnings.length > 0) setWarnPlan(plan);
      else apply(plan);
    },
    [apply],
  );

  const onToggleRule = (r: RuleInfo) => requestPlan(planRuleToggle(r, selected));
  const onToggleGroup = (g: GroupInfo) => requestPlan(planGroupToggle(g, selected));

  // ------------------------------------------------------------ analyze / clean

  const startAnalyze = async () => {
    if (!listing) return;
    const ids = orderedSelection(listing, selected);
    setSummary(null);
    setOpenResults(new Set());
    setAnalyzedFor(new Set(ids));
    await analyze.run({ ruleIds: ids });
  };

  const report = analyze.data;
  const stale = report !== null && analyzedFor !== null && !sameSet(analyzedFor, selected);
  const visible = useMemo(() => (report ? visibleResults(report.items) : []), [report]);
  const cleanableIds = useMemo(() => (report ? report.items.filter(hasContent).map((i) => i.ruleId) : []), [report]);
  const busy = analyze.loading || clean.loading;
  const canClean = !!report && !stale && cleanableIds.length > 0 && !busy;

  const finishClean = (result: CleanReport | undefined, previous: CleanReport | null) => {
    if (!result) return;
    const merged = previous ? mergeReports(previous, result) : result;
    setSummary(merged);
    analyze.reset();
    setAnalyzedFor(null);
    // Only an "ask" policy lets the user decide now; "skip" and "always" were applied server-side.
    if (settings?.closeBrowsers === 'ask' && !previous) {
      const apps = blockedApps(merged);
      if (apps.length > 0) setBlocked(apps);
    }
  };

  const doClean = async () => {
    setConfirmClean(false);
    setBlocked(null);
    const result = await clean.run({ ruleIds: cleanableIds });
    finishClean(result, null);
  };

  const retryWithClose = async () => {
    const prev = summary;
    setBlocked(null);
    if (!prev) return;
    const ids = blockedRuleIds(prev);
    const result = await clean.run({ ruleIds: ids, closeApps: 'always' });
    finishClean(result, prev);
  };

  const errors = [rules.error, analyze.error, clean.error, settingsError];
  const firstError = errors.find((e) => e !== null) ?? null;

  const progress = analyze.loading ? analyze.progress : clean.loading ? clean.progress : null;

  return (
    <div data-testid="page-clean" className="flex min-h-full flex-col gap-3 p-4">
      <ErrorBanner
        error={firstError}
        onRetry={
          rules.error
            ? () => void runRules()
            : settingsError
              ? clearError
              : undefined
        }
      />

      {!listing && rules.loading && (
        <p className="py-8 text-center text-sm text-muted" data-testid="clean-loading">
          Loading cleaning rules...
        </p>
      )}

      {listing && (
        <RuleTree
          categories={listing.categories}
          selected={selected}
          expanded={expanded}
          onToggleRule={onToggleRule}
          onToggleGroup={onToggleGroup}
          onToggleExpand={(g) => setExpanded((e) => toggled(e, g))}
        />
      )}

      {busy && (
        <Card testId="clean-progress">
          <p className="m-0 mb-2 break-words text-xs text-muted" data-testid="clean-progress-message">
            {analyze.loading ? 'Analyzing' : 'Cleaning'}
            {progress?.message ? `: ${progress.message}` : '...'}
          </p>
          <ProgressBar value={(progress?.fraction ?? 0) * 100} label={analyze.loading ? 'Analysis progress' : 'Cleaning progress'} />
          <button
            type="button"
            onClick={() => (analyze.loading ? analyze.cancel() : clean.cancel())}
            data-testid="btn-cancel"
            className="mt-2 h-9 w-full rounded-xl border border-line bg-surface-2 text-sm font-medium"
          >
            Cancel
          </button>
        </Card>
      )}

      {report && !analyze.loading && (
        <>
          {stale && (
            <p className="m-0 text-xs text-warn" data-testid="results-stale">
              The selection changed since this analysis. Analyze again before cleaning.
            </p>
          )}
          <ResultsList
            items={visible}
            expanded={openResults}
            onToggle={(id) => setOpenResults((s) => toggled(s, id))}
            totalBytes={report.totalBytes}
            totalFiles={report.totalFiles}
            totalRows={report.totalRows}
          />
        </>
      )}

      {summary && !busy && <CleanSummary report={summary} names={names} />}

      <div
        data-testid="action-bar"
        className="sticky bottom-0 z-10 mt-auto flex gap-2 rounded-xl border border-line bg-surface p-2 shadow-lg"
      >
        <button
          type="button"
          onClick={() => void startAnalyze()}
          disabled={!listing || busy || selected.size === 0}
          data-testid="btn-analyze"
          className="flex h-10 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border border-line bg-surface-2 text-sm font-semibold disabled:opacity-50"
        >
          {analyze.loading ? <Loader2 size={16} className="animate-spin" aria-hidden /> : <Sparkles size={16} aria-hidden />}
          Analyze
        </button>
        <button
          type="button"
          onClick={() => setConfirmClean(true)}
          disabled={!canClean}
          data-testid="btn-clean"
          className="flex h-10 min-w-0 flex-1 items-center justify-center gap-1.5 rounded-xl border-0 bg-accent text-sm font-semibold text-accent-fg disabled:opacity-50"
        >
          {clean.loading ? <Loader2 size={16} className="animate-spin" aria-hidden /> : <Trash2 size={16} aria-hidden />}
          Clean
        </button>
      </div>

      <ConfirmSheet
        open={warnPlan !== null}
        title={
          warnPlan && warnPlan.warnings.length === 1
            ? `Enable "${warnPlan.warnings[0]?.name}"?`
            : 'Enable items with warnings?'
        }
        message={warnPlan?.warnings.map((w) => `${w.name}: ${w.warning ?? ''}`).join(' ')}
        confirmLabel="Enable"
        danger
        onConfirm={() => {
          if (warnPlan) apply(warnPlan);
          setWarnPlan(null);
        }}
        onCancel={() => setWarnPlan(null)}
      />

      <ConfirmSheet
        open={confirmClean}
        title="Clean now?"
        message={
          report
            ? `This will permanently delete ${report.totalFiles} ${report.totalFiles === 1 ? 'file' : 'files'} (${formatBytes(report.totalBytes)})${
                report.totalRows > 0 ? ` and ${report.totalRows} browser database ${report.totalRows === 1 ? 'entry' : 'entries'}` : ''
              }. This cannot be undone.`
            : undefined
        }
        confirmLabel="Clean"
        danger
        onConfirm={() => void doClean()}
        onCancel={() => setConfirmClean(false)}
      />

      <ConfirmSheet
        open={blocked !== null}
        title="Close running apps?"
        message={
          blocked
            ? `${blocked.join(', ')} ${blocked.length === 1 ? 'is' : 'are'} running, so ${
                blocked.length === 1 ? 'its' : 'their'
              } data was not cleaned. Close ${blocked.length === 1 ? 'it' : 'them'} now and clean? Unsaved work in ${
                blocked.length === 1 ? 'it' : 'them'
              } may need saving first.`
            : undefined
        }
        confirmLabel="Close apps & clean"
        cancelLabel="Skip"
        danger
        onConfirm={() => void retryWithClose()}
        onCancel={() => setBlocked(null)}
      />
    </div>
  );
}
