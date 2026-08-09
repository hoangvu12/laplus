import type { UsageSource, UsageSummaryInput } from "@t3tools/contracts";
import type { MergedUsageSummary } from "@t3tools/shared/usage";
import { useCanGoBack, useNavigate, useRouter } from "@tanstack/react-router";
import { ArrowLeftIcon, CheckIcon, RefreshCwIcon, XIcon } from "lucide-react";
import { useMemo, useState } from "react";

import { makeUsageWindow, useUsageSummary } from "../../state/usage";
import { Skeleton } from "../ui/skeleton";
import { SidebarInset } from "../ui/sidebar";
import { UsageChartLegend, UsageProviderChart } from "./UsageProviderChart";

type Metric = "cost" | "tokens";
type Breakdown = "model" | "day";

export interface UsageReportView {
  readonly input: UsageSummaryInput;
  readonly state: "loading" | "success" | "error";
  readonly processedTokens: number;
  readonly error: string | null;
  readonly summary?: MergedUsageSummary;
  readonly sources?: ReadonlyArray<UsageSource>;
  readonly rangeDays?: 7 | 30 | 90;
  readonly onRangeChange?: (days: 7 | 30 | 90) => void;
  readonly onRefresh?: () => void;
  readonly coverageNotices?: ReadonlyArray<{ readonly label: string; readonly message: string }>;
  readonly environmentProgress?: ReadonlyArray<{
    readonly label: string;
    readonly state: "pending" | "failed" | "success";
  }>;
}

function formatDay(day: string, includeYear = true): string {
  const [year, month, date] = day.split("-").map(Number);
  return new Intl.DateTimeFormat("en-US", {
    month: "short",
    day: "numeric",
    ...(includeYear ? { year: "numeric" } : {}),
    timeZone: "UTC",
  }).format(new Date(Date.UTC(year!, month! - 1, date)));
}

function tokens(value: number): string {
  return new Intl.NumberFormat("en-US", {
    notation: value >= 100_000 ? "compact" : "standard",
    maximumFractionDigits: 1,
  }).format(value);
}

function cost(value: number): string {
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: value < 1 ? 2 : 0,
    maximumFractionDigits: value < 1 ? 4 : 2,
  }).format(value);
}

function providerName(provider: string): string {
  return provider === "claude" ? "Claude Code" : "Codex";
}

function providerColor(provider: string): string {
  return provider === "claude" ? "#d97757" : "#8b8b8b";
}

const NO_ENVIRONMENT_PROGRESS: NonNullable<UsageReportView["environmentProgress"]> = [];

function LoadingReport({
  progress = NO_ENVIRONMENT_PROGRESS,
}: {
  readonly progress?: UsageReportView["environmentProgress"];
}) {
  const pending = progress.filter((environment) => environment.state === "pending").length;
  return (
    <div className="space-y-6" aria-label="Usage is loading">
      {progress.length > 1 ? (
        <div className="rounded-lg border border-border p-3 text-sm">
          <div className="flex flex-wrap gap-3">
            {progress.map((environment) => (
              <span key={environment.label} className="flex items-center gap-1.5">
                {environment.state === "success" ? (
                  <CheckIcon className="size-4 text-emerald-500" />
                ) : null}
                {environment.state === "failed" ? (
                  <XIcon className="size-4 text-destructive" />
                ) : null}
                {environment.state === "pending" ? (
                  <span className="size-2 animate-pulse rounded-full bg-amber-500" />
                ) : null}
                {environment.label}
                {environment.state === "pending" ? "…" : ""}
              </span>
            ))}
          </div>
          {pending > 0 ? (
            <p className="mt-2 text-muted-foreground">
              {pending} {pending === 1 ? "device" : "devices"} still scanning
            </p>
          ) : null}
        </div>
      ) : null}
      <div className="grid gap-4 md:grid-cols-3">
        <Skeleton className="h-36 md:col-span-2" />
        <Skeleton className="h-36" />
      </div>
      <Skeleton className="h-72" />
      <div className="grid grid-cols-2 gap-3 md:grid-cols-5">
        {Array.from({ length: 5 }, (_, index) => (
          <Skeleton key={index} className="h-24" />
        ))}
      </div>
    </div>
  );
}

function ProviderRows({
  summary,
  metric,
}: {
  readonly summary: MergedUsageSummary;
  readonly metric: Metric;
}) {
  const providers = [...summary.byProvider].sort((left, right) =>
    metric === "cost"
      ? right.totals.costUsd - left.totals.costUsd
      : right.totals.processedTokens - left.totals.processedTokens,
  );
  return (
    <section
      aria-labelledby="provider-heading"
      className="rounded-xl border border-border bg-card p-5"
    >
      <h2 id="provider-heading" className="font-medium">
        Providers
      </h2>
      <div className="mt-4 space-y-5">
        {providers.map((provider) => {
          const share = metric === "cost" ? provider.costShare : provider.tokenShare;
          return (
            <div key={provider.provider}>
              <div className="flex items-center justify-between gap-4">
                <span className="flex items-center gap-2">
                  <span
                    className="size-2.5 rounded-full"
                    style={{ backgroundColor: providerColor(provider.provider) }}
                  />
                  {providerName(provider.provider)}
                </span>
                <strong className="tabular-nums">
                  {metric === "cost"
                    ? cost(provider.totals.costUsd)
                    : tokens(provider.totals.processedTokens)}
                </strong>
              </div>
              <div className="mt-2 h-2 overflow-hidden rounded-full bg-muted">
                <div
                  className="h-full rounded-full"
                  style={{
                    width: `${Math.max(0, Math.min(100, share * 100))}%`,
                    backgroundColor: providerColor(provider.provider),
                  }}
                />
              </div>
              <p className="mt-1 text-xs text-muted-foreground">
                {Math.round(share * 100)}% of {metric} ·{" "}
                {metric === "cost"
                  ? `${tokens(provider.totals.processedTokens)} tokens`
                  : cost(provider.totals.costUsd)}
              </p>
            </div>
          );
        })}
      </div>
    </section>
  );
}

function Metrics({ summary }: { readonly summary: MergedUsageSummary }) {
  const totals = summary.totals;
  const observedInput =
    totals.uncachedInputTokens + totals.cachedInputTokens + totals.cacheCreationTokens;
  const activeDays = Math.max(1, summary.byDay.filter((day) => day.totals.records > 0).length);
  const cards = [
    [
      "Processed tokens",
      tokens(totals.processedTokens),
      `${tokens(Math.round(totals.processedTokens / activeDays))} per active day`,
    ],
    [
      "Cached input",
      tokens(totals.cachedInputTokens),
      `${observedInput === 0 ? 0 : Math.round((totals.cachedInputTokens / observedInput) * 100)}% of observed input`,
    ],
    [
      "Uncached input",
      tokens(totals.uncachedInputTokens),
      `${tokens(totals.cacheCreationTokens)} cache writes`,
    ],
    ["Output", tokens(totals.outputTokens), `includes ${tokens(totals.reasoningTokens)} reasoning`],
    ["Cache savings", cost(totals.cacheSavingsUsd), "vs full input rates"],
  ] as const;
  return (
    <section aria-label="Usage metrics" className="grid grid-cols-2 gap-3 md:grid-cols-5">
      {cards.map(([label, value, detail]) => (
        <div key={label} className="rounded-xl border border-border bg-card p-4">
          <p className="text-xs text-muted-foreground">{label}</p>
          <p className="mt-2 text-xl font-semibold tabular-nums">{value}</p>
          <p className="mt-1 text-xs text-muted-foreground">{detail}</p>
        </div>
      ))}
    </section>
  );
}

function BreakdownTable({
  summary,
  breakdown,
}: {
  readonly summary: MergedUsageSummary;
  readonly breakdown: Breakdown;
}) {
  const models = useMemo(() => {
    const grouped = new Map<
      string,
      { provider: string; model: string; costUsd: number; processedTokens: number }
    >();
    for (const bucket of summary.buckets) {
      const key = `${bucket.provider}:${bucket.model}`;
      const previous = grouped.get(key) ?? {
        provider: bucket.provider,
        model: bucket.model,
        costUsd: 0,
        processedTokens: 0,
      };
      previous.costUsd += bucket.costUsd;
      previous.processedTokens +=
        bucket.totals.uncachedInputTokens +
        bucket.totals.cachedInputTokens +
        bucket.totals.cacheCreationTokens +
        bucket.totals.outputTokens;
      grouped.set(key, previous);
    }
    return [...grouped.values()].sort(
      (left, right) => right.costUsd - left.costUsd || right.processedTokens - left.processedTokens,
    );
  }, [summary]);
  const days = [...summary.byDay]
    .sort((left, right) => right.day.localeCompare(left.day))
    .slice(0, 8);
  const rows = breakdown === "model" ? models : days;
  return (
    <div className="overflow-x-auto rounded-xl border border-border">
      <table className="w-full min-w-[560px] text-sm">
        <thead className="bg-muted/50 text-left text-xs text-muted-foreground">
          <tr>
            {breakdown === "model" ? (
              <>
                <th className="p-3">Model</th>
                <th>Cost</th>
                <th>Share</th>
                <th>Tokens</th>
              </>
            ) : (
              <>
                <th className="p-3">Day</th>
                <th>Codex</th>
                <th>Claude Code</th>
                <th>Total</th>
                <th>Tokens</th>
              </>
            )}
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 ? (
            <tr>
              <td className="p-6 text-center text-muted-foreground" colSpan={5}>
                No activity in this window.
              </td>
            </tr>
          ) : breakdown === "model" ? (
            models.map((model) => (
              <tr key={`${model.provider}:${model.model}`} className="border-t border-border">
                <td className="p-3">
                  <span
                    className="mr-2 inline-block size-2 rounded-full"
                    style={{ backgroundColor: providerColor(model.provider) }}
                  />
                  {model.model}
                </td>
                <td>{cost(model.costUsd)}</td>
                <td>
                  {summary.totals.costUsd === 0
                    ? "—"
                    : `${Math.round((model.costUsd / summary.totals.costUsd) * 100)}%`}
                </td>
                <td>{tokens(model.processedTokens)}</td>
              </tr>
            ))
          ) : (
            days.map((day) => {
              const buckets = summary.buckets.filter((bucket) => bucket.day === day.day);
              const providerCost = (provider: string) =>
                buckets
                  .filter((bucket) => bucket.provider === provider)
                  .reduce((total, bucket) => total + bucket.costUsd, 0);
              return (
                <tr key={day.day} className="border-t border-border">
                  <td className="p-3">{formatDay(day.day, false)}</td>
                  <td>{cost(providerCost("codex"))}</td>
                  <td>{cost(providerCost("claude"))}</td>
                  <td>{cost(day.totals.costUsd)}</td>
                  <td>{tokens(day.totals.processedTokens)}</td>
                </tr>
              );
            })
          )}
        </tbody>
      </table>
    </div>
  );
}

export function UsageReport({ view }: { readonly view: UsageReportView }) {
  const canGoBack = useCanGoBack();
  const navigate = useNavigate();
  const router = useRouter();
  const [metric, setMetric] = useState<Metric>("cost");
  const [breakdown, setBreakdown] = useState<Breakdown>("model");
  const summary = view.summary;
  return (
    <SidebarInset className="h-dvh min-h-0 overflow-auto bg-background text-foreground">
      <main className="mx-auto flex w-full max-w-6xl flex-col gap-6 px-4 py-6 sm:px-6">
        <header className="flex flex-wrap items-start gap-3">
          <button
            type="button"
            aria-label="Back"
            className="mt-1 rounded-md border border-border p-2 text-muted-foreground hover:text-foreground"
            onClick={() => {
              if (canGoBack) router.history.back();
              else void navigate({ to: "/" });
            }}
          >
            <ArrowLeftIcon className="size-4" />
          </button>
          <div>
            <h1 className="text-2xl font-semibold">Usage</h1>
            <p className="mt-1 text-sm text-muted-foreground">
              {formatDay(view.input.sinceDay)} to {formatDay(view.input.untilDay)}
            </p>
          </div>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            <button
              type="button"
              aria-label="Refresh usage"
              onClick={view.onRefresh}
              className="rounded-md border border-border p-2 text-muted-foreground hover:text-foreground"
            >
              <RefreshCwIcon className="size-4" />
            </button>
            <div
              role="group"
              aria-label="Usage range"
              className="flex rounded-md border border-border p-1"
            >
              {([7, 30, 90] as const).map((days) => (
                <button
                  key={days}
                  type="button"
                  aria-pressed={view.rangeDays === days}
                  onClick={() => view.onRangeChange?.(days)}
                  className="rounded px-3 py-1 text-sm aria-pressed:bg-muted"
                >
                  {days} days
                </button>
              ))}
            </div>
          </div>
        </header>
        {view.state === "loading" ? <LoadingReport progress={view.environmentProgress} /> : null}
        {view.state === "error" ? (
          <section className="rounded-xl border border-destructive/30 bg-destructive/5 p-5 text-destructive">
            {view.error}
          </section>
        ) : null}
        {view.state === "success" ? (
          <>
            {view.sources?.length ? (
              <section
                aria-label="Usage source coverage"
                className="flex flex-wrap gap-2 text-sm text-muted-foreground"
              >
                {view.sources.map((source) => (
                  <span
                    key={`${source.fingerprint.provider}:${source.fingerprint.resolvedHomePath}`}
                    className="rounded-full border border-border px-3 py-1"
                  >
                    {source.fingerprint.provider === "claude" ? "Claude" : "Codex"}: {source.status}
                  </span>
                ))}
              </section>
            ) : null}
            {view.coverageNotices?.length ? (
              <section
                aria-label="Usage coverage notices"
                className="rounded-lg border border-amber-500/30 bg-amber-500/5 p-3 text-sm"
              >
                {view.coverageNotices.map((notice) => (
                  <p key={`${notice.label}:${notice.message}`}>
                    <strong>{notice.label}:</strong> {notice.message}
                  </p>
                ))}
              </section>
            ) : null}
            {summary?.pricing.status === "unavailable" ? (
              <p className="text-sm text-amber-700 dark:text-amber-300">
                API-equivalent pricing is unavailable; token totals remain complete.
              </p>
            ) : null}
            {(summary?.totals.unpricedRecords ?? 0) > 0 ? (
              <p className="text-sm text-muted-foreground">
                {summary!.totals.unpricedRecords} usage{" "}
                {summary!.totals.unpricedRecords === 1 ? "record is" : "records are"} unpriced.
              </p>
            ) : null}
            <div className="grid gap-4 md:grid-cols-3">
              <section className="rounded-xl border border-border bg-card p-5 md:col-span-2">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div>
                    <p className="text-xs font-medium tracking-wide text-muted-foreground uppercase">
                      {metric === "cost" ? "Raw token cost" : "Processed tokens"}
                    </p>
                    <p className="mt-2 text-4xl font-semibold tabular-nums">
                      {metric === "cost"
                        ? `${cost(summary?.totals.costUsd ?? 0)}*`
                        : tokens(summary?.totals.processedTokens ?? view.processedTokens)}
                    </p>
                    <p className="mt-2 text-sm text-muted-foreground">
                      {metric === "cost"
                        ? "* if billed at full API rate"
                        : `Input, cache reads and output across ${summary?.distinctSessions ?? 0} sessions.`}
                    </p>
                  </div>
                  <div
                    role="group"
                    aria-label="Chart metric"
                    className="flex rounded-md border border-border p-1"
                  >
                    <button
                      type="button"
                      aria-pressed={metric === "cost"}
                      onClick={() => setMetric("cost")}
                      className="rounded px-3 py-1 text-xs aria-pressed:bg-muted"
                    >
                      COST
                    </button>
                    <button
                      type="button"
                      aria-pressed={metric === "tokens"}
                      onClick={() => setMetric("tokens")}
                      className="rounded px-3 py-1 text-xs aria-pressed:bg-muted"
                    >
                      TOKENS
                    </button>
                  </div>
                </div>
              </section>
              {summary ? <ProviderRows summary={summary} metric={metric} /> : null}
            </div>
            {summary ? (
              <>
                <section className="rounded-xl border border-border bg-card p-5">
                  <h2 className="font-medium">
                    Daily {metric === "cost" ? "cost" : "processed tokens"}
                  </h2>
                  <UsageProviderChart
                    sinceDay={summary.sinceDay}
                    untilDay={summary.untilDay}
                    buckets={summary.buckets}
                    metric={metric}
                  />
                  <UsageChartLegend />
                </section>
                <Metrics summary={summary} />
                <section aria-labelledby="breakdown-heading">
                  <div className="mb-3 flex items-center justify-between">
                    <h2 id="breakdown-heading" className="font-medium">
                      Breakdown
                    </h2>
                    <div
                      role="group"
                      aria-label="Breakdown"
                      className="flex rounded-md border border-border p-1"
                    >
                      <button
                        type="button"
                        aria-pressed={breakdown === "model"}
                        onClick={() => setBreakdown("model")}
                        className="rounded px-3 py-1 text-xs aria-pressed:bg-muted"
                      >
                        MODEL
                      </button>
                      <button
                        type="button"
                        aria-pressed={breakdown === "day"}
                        onClick={() => setBreakdown("day")}
                        className="rounded px-3 py-1 text-xs aria-pressed:bg-muted"
                      >
                        DAY
                      </button>
                    </div>
                  </div>
                  <BreakdownTable summary={summary} breakdown={breakdown} />
                </section>
              </>
            ) : null}
          </>
        ) : null}
      </main>
    </SidebarInset>
  );
}

export function UsagePage() {
  const [rangeDays, setRangeDays] = useState<7 | 30 | 90>(30);
  const input = useMemo(() => makeUsageWindow(rangeDays), [rangeDays]);
  const usage = useUsageSummary(input);
  const view: UsageReportView = {
    input,
    state: usage.error ? "error" : usage.summary ? "success" : "loading",
    processedTokens: usage.summary?.totals.processedTokens ?? 0,
    error: usage.error,
    ...(usage.summary ? { summary: usage.summary, sources: usage.summary.sources } : {}),
    rangeDays,
    onRangeChange: setRangeDays,
    onRefresh: usage.refresh,
    coverageNotices: usage.notices,
    environmentProgress: usage.environmentProgress,
  };
  return <UsageReport view={view} />;
}
