import {
  USAGE_CONTRACT_VERSION,
  type UsageBucket,
  type UsageCostSource,
  type UsagePricing,
  type UsageProviderKind,
  type UsageSource,
  type UsageSummary,
  type UsageTokenTotals,
} from "@t3tools/contracts";

export type EnvironmentUsageResult =
  | {
      readonly environmentId: string;
      readonly label: string;
      readonly state: "pending";
    }
  | {
      readonly environmentId: string;
      readonly label: string;
      readonly state: "failed";
      readonly message?: string;
    }
  | {
      readonly environmentId: string;
      readonly label: string;
      readonly state: "success";
      readonly summary: UsageSummary;
    };

export type UsageCoverageNoticeKind =
  | "duplicate"
  | "failed"
  | "incompatible"
  | "missing"
  | "partial";

export interface UsageCoverageNotice {
  readonly kind: UsageCoverageNoticeKind;
  readonly label: string;
  readonly message: string;
}

export interface UsageMetricTotals extends UsageTokenTotals {
  readonly processedTokens: number;
  readonly costUsd: number;
  readonly cacheSavingsUsd: number;
  readonly records: number;
  readonly unpricedRecords: number;
}

export interface UsageBreakdown {
  readonly key: string;
  readonly totals: UsageMetricTotals;
  readonly tokenShare: number;
  readonly costShare: number;
}

export interface MergedUsageSummary extends UsageSummary {
  readonly totals: UsageMetricTotals;
  readonly distinctSessions: number;
  readonly byProvider: ReadonlyArray<UsageBreakdown & { readonly provider: UsageProviderKind }>;
  readonly byModel: ReadonlyArray<
    UsageBreakdown & { readonly provider: UsageProviderKind; readonly model: string }
  >;
  readonly byDay: ReadonlyArray<UsageBreakdown & { readonly day: string }>;
}

export interface MergedUsageResult {
  readonly isPending: boolean;
  readonly summary: MergedUsageSummary | null;
  readonly notices: ReadonlyArray<UsageCoverageNotice>;
}

const ZERO_TOKENS: UsageTokenTotals = {
  uncachedInputTokens: 0,
  cachedInputTokens: 0,
  cacheCreationTokens: 0,
  outputTokens: 0,
  reasoningTokens: 0,
};

function boundedMessage(value: string): string {
  return value.slice(0, 240);
}

function safeSource(source: UsageSource): UsageSource {
  return {
    ...source,
    message:
      source.status === "ok" || source.status === "missing"
        ? null
        : `${source.fingerprint.provider} usage source is ${source.status}.`,
  };
}

function fingerprintKey(source: UsageSource): string {
  const fingerprint = source.fingerprint;
  return JSON.stringify([
    fingerprint.hostId,
    fingerprint.provider,
    fingerprint.resolvedHomePath,
    fingerprint.volumeId,
  ]);
}

function bucketKey(bucket: UsageBucket): string {
  return JSON.stringify([bucket.day, bucket.provider, bucket.model]);
}

function addTokens(left: UsageTokenTotals, right: UsageTokenTotals): UsageTokenTotals {
  return {
    uncachedInputTokens: left.uncachedInputTokens + right.uncachedInputTokens,
    cachedInputTokens: left.cachedInputTokens + right.cachedInputTokens,
    cacheCreationTokens: left.cacheCreationTokens + right.cacheCreationTokens,
    outputTokens: left.outputTokens + right.outputTokens,
    reasoningTokens: left.reasoningTokens + right.reasoningTokens,
  };
}

function mergedCostSource(left: UsageCostSource, right: UsageCostSource): UsageCostSource {
  return left === right ? left : "modelPriced";
}

function mergeBucket(left: UsageBucket, right: UsageBucket): UsageBucket {
  return {
    ...left,
    totals: addTokens(left.totals, right.totals),
    costUsd: left.costUsd + right.costUsd,
    cacheSavingsUsd: left.cacheSavingsUsd + right.cacheSavingsUsd,
    costSource: mergedCostSource(left.costSource, right.costSource),
    records: left.records + right.records,
    unpricedRecords: left.unpricedRecords + right.unpricedRecords,
    sessions: left.sessions + right.sessions,
  };
}

function metricTotals(buckets: ReadonlyArray<UsageBucket>): UsageMetricTotals {
  const totals = buckets.reduce(
    (accumulator, bucket) => ({
      tokens: addTokens(accumulator.tokens, bucket.totals),
      costUsd: accumulator.costUsd + bucket.costUsd,
      cacheSavingsUsd: accumulator.cacheSavingsUsd + bucket.cacheSavingsUsd,
      records: accumulator.records + bucket.records,
      unpricedRecords: accumulator.unpricedRecords + bucket.unpricedRecords,
    }),
    { tokens: ZERO_TOKENS, costUsd: 0, cacheSavingsUsd: 0, records: 0, unpricedRecords: 0 },
  );
  return {
    ...totals.tokens,
    processedTokens:
      totals.tokens.uncachedInputTokens +
      totals.tokens.cachedInputTokens +
      totals.tokens.cacheCreationTokens +
      totals.tokens.outputTokens,
    costUsd: totals.costUsd,
    cacheSavingsUsd: totals.cacheSavingsUsd,
    records: totals.records,
    unpricedRecords: totals.unpricedRecords,
  };
}

function breakdown<K extends string>(
  buckets: ReadonlyArray<UsageBucket>,
  keyOf: (bucket: UsageBucket) => K,
  total: UsageMetricTotals,
): ReadonlyArray<UsageBreakdown> {
  const grouped = new Map<string, UsageBucket[]>();
  for (const bucket of buckets) {
    const key = keyOf(bucket);
    grouped.set(key, [...(grouped.get(key) ?? []), bucket]);
  }
  return [...grouped.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([key, values]) => {
      const totals = metricTotals(values);
      return {
        key,
        totals,
        tokenShare:
          total.processedTokens === 0 ? 0 : totals.processedTokens / total.processedTokens,
        costShare: total.costUsd === 0 ? 0 : totals.costUsd / total.costUsd,
      };
    });
}

function weakestPricing(pricing: ReadonlyArray<UsagePricing>): UsagePricing {
  const rank: Record<UsagePricing["status"], number> = { fresh: 0, cached: 1, unavailable: 2 };
  return pricing.reduce((weakest, candidate) =>
    rank[candidate.status] > rank[weakest.status] ? candidate : weakest,
  );
}

export function mergeUsageEnvironments(
  environments: ReadonlyArray<EnvironmentUsageResult>,
): MergedUsageResult {
  if (environments.some((environment) => environment.state === "pending")) {
    return { isPending: true, summary: null, notices: [] };
  }

  const notices: UsageCoverageNotice[] = [];
  const compatible: Array<Extract<EnvironmentUsageResult, { state: "success" }>> = [];
  for (const environment of environments) {
    if (environment.state === "pending") {
      continue;
    } else if (environment.state === "failed") {
      notices.push({
        kind: "failed",
        label: environment.label,
        message: "Usage could not be read from this environment.",
      });
    } else if (environment.summary.contractVersion !== USAGE_CONTRACT_VERSION) {
      notices.push({
        kind: "incompatible",
        label: environment.label,
        message: `Usage contract version ${environment.summary.contractVersion} is incompatible.`,
      });
    } else {
      compatible.push(environment);
    }
  }

  compatible.sort((left, right) => left.environmentId.localeCompare(right.environmentId));
  const sourceRank = { ok: 0, partial: 1, failed: 2 } as const;
  const sourceOwners = new Map<string, { environmentId: string; label: string; rank: number }>();
  for (const environment of compatible) {
    for (const source of environment.summary.sources) {
      if (source.status === "missing") continue;
      const key = fingerprintKey(source);
      const candidate = {
        environmentId: environment.environmentId,
        label: environment.label,
        rank: sourceRank[source.status],
      };
      const owner = sourceOwners.get(key);
      if (
        owner === undefined ||
        candidate.rank < owner.rank ||
        (candidate.rank === owner.rank && candidate.environmentId < owner.environmentId)
      ) {
        sourceOwners.set(key, candidate);
      }
    }
  }
  const sources: UsageSource[] = [];
  const buckets = new Map<string, UsageBucket>();
  for (const environment of compatible) {
    const acceptedProviders = new Set<UsageProviderKind>();
    for (const source of environment.summary.sources) {
      const key = fingerprintKey(source);
      const owner = source.status === "missing" ? undefined : sourceOwners.get(key);
      const isOwner = owner?.environmentId === environment.environmentId;
      if (owner !== undefined && !isOwner) {
        notices.push({
          kind: "duplicate",
          label: environment.label,
          message: boundedMessage(
            `${source.fingerprint.provider} source duplicates ${owner.label}.`,
          ),
        });
      }
      if (source.status === "missing" || source.status === "partial") {
        notices.push({
          kind: source.status,
          label: environment.label,
          message: `${source.fingerprint.provider} usage source is ${source.status}.`,
        });
      } else if (source.status === "failed") {
        notices.push({
          kind: "failed",
          label: environment.label,
          message: `${source.fingerprint.provider} usage source failed.`,
        });
      }
      if (source.status === "missing" || !isOwner) continue;
      sources.push(safeSource(source));
      acceptedProviders.add(source.fingerprint.provider);
    }
    for (const bucket of environment.summary.buckets) {
      if (!acceptedProviders.has(bucket.provider)) continue;
      const key = bucketKey(bucket);
      const previous = buckets.get(key);
      buckets.set(key, previous === undefined ? bucket : mergeBucket(previous, bucket));
    }
  }

  const orderedBuckets = [...buckets.values()].sort(
    (left, right) =>
      left.day.localeCompare(right.day) ||
      left.provider.localeCompare(right.provider) ||
      left.model.localeCompare(right.model),
  );
  const totals = metricTotals(orderedBuckets);
  const first = compatible[0]?.summary;
  if (first === undefined) {
    return { isPending: false, summary: null, notices };
  }
  const pricing = weakestPricing(compatible.map((environment) => environment.summary.pricing));
  const byProvider = breakdown(orderedBuckets, (bucket) => bucket.provider, totals).map(
    (entry) => ({
      ...entry,
      provider: entry.key as UsageProviderKind,
    }),
  );
  const byModel = breakdown(
    orderedBuckets,
    (bucket) => JSON.stringify([bucket.provider, bucket.model]),
    totals,
  ).map((entry) => {
    const [provider, model] = JSON.parse(entry.key) as [UsageProviderKind, string];
    return { ...entry, provider, model };
  });
  const byDay = breakdown(orderedBuckets, (bucket) => bucket.day, totals).map((entry) => ({
    ...entry,
    day: entry.key,
  }));
  return {
    isPending: false,
    notices,
    summary: {
      contractVersion: USAGE_CONTRACT_VERSION,
      readAt:
        compatible
          .map(({ summary }) => summary.readAt)
          .sort()
          .at(-1) ?? first.readAt,
      timeZone: first.timeZone,
      sinceDay: first.sinceDay,
      untilDay: first.untilDay,
      buckets: orderedBuckets,
      sources,
      pricing,
      scanDurationMs: compatible.reduce(
        (total, environment) => total + environment.summary.scanDurationMs,
        0,
      ),
      totals,
      distinctSessions: sources.reduce((total, source) => total + source.distinctSessions, 0),
      byProvider,
      byModel,
      byDay,
    },
  };
}
