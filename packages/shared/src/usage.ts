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
  readonly byModel: ReadonlyArray<UsageBreakdown & { readonly model: string }>;
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

function weakestCostSource(left: UsageCostSource, right: UsageCostSource): UsageCostSource {
  const rank: Record<UsageCostSource, number> = {
    providerReported: 0,
    modelPriced: 1,
    unpriced: 2,
  };
  return rank[left] >= rank[right] ? left : right;
}

function mergeBucket(left: UsageBucket, right: UsageBucket): UsageBucket {
  return {
    ...left,
    totals: addTokens(left.totals, right.totals),
    costUsd: left.costUsd + right.costUsd,
    cacheSavingsUsd: left.cacheSavingsUsd + right.cacheSavingsUsd,
    costSource: weakestCostSource(left.costSource, right.costSource),
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

  const seenSources = new Map<string, string>();
  const sources: UsageSource[] = [];
  const buckets = new Map<string, UsageBucket>();
  for (const environment of compatible) {
    const acceptedProviders = new Set<UsageProviderKind>();
    for (const source of environment.summary.sources) {
      const key = fingerprintKey(source);
      const owner = seenSources.get(key);
      if (owner !== undefined) {
        notices.push({
          kind: "duplicate",
          label: environment.label,
          message: boundedMessage(`${source.fingerprint.provider} source duplicates ${owner}.`),
        });
        continue;
      }
      seenSources.set(key, environment.label);
      sources.push(safeSource(source));
      acceptedProviders.add(source.fingerprint.provider);
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
  const byModel = breakdown(orderedBuckets, (bucket) => bucket.model, totals).map((entry) => ({
    ...entry,
    model: entry.key,
  }));
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
