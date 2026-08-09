import { useAtomRefresh, useAtomValue } from "@effect/atom-react";
import type { EnvironmentId, UsageDay, UsageSummary, UsageSummaryInput } from "@t3tools/contracts";
import * as Option from "effect/Option";
import { AsyncResult } from "effect/unstable/reactivity";
import { useMemo } from "react";

import { serverEnvironment } from "./server";
import { usePrimaryEnvironmentId } from "./environments";

export function makeUsageWindow(days = 30, now = new Date()): UsageSummaryInput {
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  const until = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const since = new Date(until.getFullYear(), until.getMonth(), until.getDate() - days + 1);
  const day = (value: Date) =>
    `${value.getFullYear()}-${String(value.getMonth() + 1).padStart(2, "0")}-${String(value.getDate()).padStart(2, "0")}` as UsageDay;
  return { sinceDay: day(since), untilDay: day(until), timeZone };
}

export function processedTokenTotal(summary: UsageSummary): number {
  return summary.buckets.reduce(
    (total, bucket) =>
      total +
      bucket.totals.uncachedInputTokens +
      bucket.totals.cachedInputTokens +
      bucket.totals.cacheCreationTokens +
      bucket.totals.outputTokens,
    0,
  );
}

export function useUsageSummary(input: UsageSummaryInput) {
  const environmentId = usePrimaryEnvironmentId();
  const target = useMemo(
    () => ({ environmentId: (environmentId ?? "missing-primary") as EnvironmentId, input }),
    [environmentId, input],
  );
  const atom = serverEnvironment.usageSummary(target);
  const result = useAtomValue(atom);
  const refresh = useAtomRefresh(atom);
  const summary = Option.getOrNull(AsyncResult.value(result));
  return {
    summary,
    isPending: environmentId !== null && result.waiting,
    error: result._tag === "Failure" ? "Usage could not be loaded." : null,
    refresh,
  };
}
