import type { UsageSummaryInput } from "@t3tools/contracts";
import { useCanGoBack, useNavigate, useRouter } from "@tanstack/react-router";
import { ArrowLeftIcon } from "lucide-react";
import { useMemo } from "react";

import { SidebarInset } from "../ui/sidebar";
import { makeUsageWindow, processedTokenTotal, useUsageSummary } from "../../state/usage";

export interface UsageReportView {
  readonly input: UsageSummaryInput;
  readonly state: "loading" | "success" | "error";
  readonly processedTokens: number;
  readonly error: string | null;
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
        <header className="flex items-start gap-3">
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
        </header>
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
  const input = useMemo(() => makeUsageWindow(30), []);
  const usage = useUsageSummary(input);
  const view: UsageReportView = {
    input,
    state: usage.error ? "error" : usage.summary ? "success" : "loading",
    processedTokens: usage.summary ? processedTokenTotal(usage.summary) : 0,
    error: usage.error,
  };
  return <UsageReport view={view} />;
}
