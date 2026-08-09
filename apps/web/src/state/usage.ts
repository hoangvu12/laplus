import { RegistryContext, useAtomValue } from "@effect/atom-react";
import type { EnvironmentId, UsageDay, UsageSummaryInput } from "@t3tools/contracts";
import { mergeUsageEnvironments, type EnvironmentUsageResult } from "@t3tools/shared/usage";
import * as Option from "effect/Option";
import { AsyncResult, Atom } from "effect/unstable/reactivity";
import { useCallback, useContext, useMemo } from "react";

import { useEnvironments } from "./environments";
import { serverEnvironment } from "./server";

export function makeUsageWindow(days = 30, now = new Date()): UsageSummaryInput {
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  const until = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const since = new Date(until.getFullYear(), until.getMonth(), until.getDate() - days + 1);
  const day = (value: Date) =>
    `${value.getFullYear()}-${String(value.getMonth() + 1).padStart(2, "0")}-${String(value.getDate()).padStart(2, "0")}` as UsageDay;
  return { sinceDay: day(since), untilDay: day(until), timeZone };
}

interface UsageTarget {
  readonly environmentId: EnvironmentId;
  readonly label: string;
  readonly phase: string;
}

export function useUsageSummary(input: UsageSummaryInput) {
  const catalog = useEnvironments();
  const targets = useMemo<ReadonlyArray<UsageTarget>>(
    () =>
      catalog.environments.map((environment) => ({
        environmentId: environment.environmentId,
        label: environment.label,
        phase: environment.connection.phase,
      })),
    [catalog.environments],
  );
  const targetsKey = targets
    .map((target) => `${target.environmentId}:${target.label}:${target.phase}`)
    .join("|");
  const registry = useContext(RegistryContext);
  const queries = useMemo(
    () =>
      targets.map((target) => ({
        target,
        atom: serverEnvironment.usageSummary({ environmentId: target.environmentId, input }),
      })),
    [input, targetsKey],
  );
  const aggregateAtom = useMemo(
    () =>
      Atom.make((get) => queries.map(({ target, atom }) => ({ target, result: get(atom) }))).pipe(
        Atom.withLabel(`web-usage-environments:${targetsKey}`),
      ),
    [queries, targetsKey],
  );
  const queried = useAtomValue(aggregateAtom);
  const refresh = useCallback(() => {
    for (const query of queries) registry.refresh(query.atom);
  }, [queries, registry]);
  const environments: ReadonlyArray<EnvironmentUsageResult> = queried.map(({ target, result }) => {
    if (target.phase === "offline" || target.phase === "error") {
      return { environmentId: target.environmentId, label: target.label, state: "failed" };
    }
    const summary = Option.getOrNull(AsyncResult.value(result));
    if (summary !== null) {
      return {
        environmentId: target.environmentId,
        label: target.label,
        state: "success",
        summary,
      };
    }
    if (result._tag === "Failure") {
      return { environmentId: target.environmentId, label: target.label, state: "failed" };
    }
    return { environmentId: target.environmentId, label: target.label, state: "pending" };
  });
  const merged = mergeUsageEnvironments(environments);
  const awaitingCatalog = !catalog.isReady || targets.length === 0;
  return {
    summary: merged.summary,
    notices: merged.notices,
    isPending: awaitingCatalog || merged.isPending,
    error:
      !awaitingCatalog && !merged.isPending && merged.summary === null
        ? "Usage could not be loaded from any connected environment."
        : null,
    refresh,
    environmentProgress: environments.map((environment) => ({
      label: environment.label,
      state: environment.state,
    })),
  };
}
