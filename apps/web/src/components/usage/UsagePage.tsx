import type { UsageSource, UsageSummaryInput } from "@t3tools/contracts";
import { useCanGoBack, useNavigate, useRouter } from "@tanstack/react-router";
import { ArrowLeftIcon, RefreshCwIcon } from "lucide-react";
import { useMemo, useState } from "react";

import { SidebarInset } from "../ui/sidebar";
import { makeUsageWindow, processedTokenTotal, useUsageSummary } from "../../state/usage";

export interface UsageReportView {
  readonly input: UsageSummaryInput;
  readonly state: "loading" | "success" | "error";
  readonly processedTokens: number;
  readonly error: string | null;
  readonly sources?: ReadonlyArray<UsageSource>;
  readonly rangeDays?: 7 | 30 | 90;
  readonly onRangeChange?: (days: 7 | 30 | 90) => void;
  readonly onRefresh?: () => void;
}

function formatDay(day: string): string {
  const [year, month, date] = day.split("-").map(Number);
  return new Intl.DateTimeFormat("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
    timeZone: "UTC",
  }).format(new Date(Date.UTC(year!, month! - 1, date)));
}

export function UsageReport({ view }: { readonly view: UsageReportView }) {
  const canGoBack = useCanGoBack();
  const navigate = useNavigate();
  const router = useRouter();
  return (
    <SidebarInset className="h-dvh min-h-0 overflow-auto bg-background text-foreground">
      <main className="mx-auto flex w-full max-w-4xl flex-col gap-8 px-4 py-6 sm:px-6">
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
        <section className="rounded-xl border border-border bg-card p-5 sm:p-6">
          <p className="text-xs font-medium tracking-wide text-muted-foreground uppercase">
            Processed tokens
          </p>
          {view.state === "loading" ? (
            <p className="mt-3 text-sm text-muted-foreground">Loading usage…</p>
          ) : null}
          {view.state === "error" ? (
            <p className="mt-3 text-sm text-destructive">{view.error}</p>
          ) : null}
          {view.state === "success" ? (
            <p className="mt-2 text-4xl font-semibold tabular-nums">
              {view.processedTokens.toLocaleString()}
            </p>
          ) : null}
        </section>
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
    processedTokens: usage.summary ? processedTokenTotal(usage.summary) : 0,
    error: usage.error,
    ...(usage.summary ? { sources: usage.summary.sources } : {}),
    rangeDays,
    onRangeChange: setRangeDays,
    onRefresh: usage.refresh,
  };
  return <UsageReport view={view} />;
}
