import { USAGE_CONTRACT_VERSION, type UsageSummary } from "@t3tools/contracts";
import { describe, expect, it } from "vite-plus/test";

import { mergeUsageEnvironments, type EnvironmentUsageResult } from "./usage.ts";

function summary(overrides: Partial<UsageSummary> = {}): UsageSummary {
  return {
    contractVersion: USAGE_CONTRACT_VERSION,
    readAt: "2026-08-09T12:00:00Z",
    timeZone: "UTC",
    sinceDay: "2026-08-03" as never,
    untilDay: "2026-08-09" as never,
    buckets: [],
    sources: [],
    pricing: { status: "fresh", source: "LiteLLM", fetchedAt: "2026-08-09", knownModels: 2 },
    scanDurationMs: 1,
    ...overrides,
  };
}

function source(
  provider: "claude" | "codex",
  overrides: Partial<UsageSummary["sources"][number]> = {},
): UsageSummary["sources"][number] {
  return {
    fingerprint: {
      hostId: "host-a",
      provider,
      resolvedHomePath: `/home/me/.${provider}`,
      volumeId: "1:2",
    },
    status: "ok",
    scannedFiles: 1,
    skippedFiles: 0,
    malformedRecords: 0,
    distinctSessions: 1,
    message: null,
    ...overrides,
  };
}

function bucket(
  provider: "claude" | "codex",
  model: string,
  overrides: Partial<UsageSummary["buckets"][number]> = {},
): UsageSummary["buckets"][number] {
  return {
    day: "2026-08-09" as never,
    provider,
    model,
    totals: {
      uncachedInputTokens: 10,
      cachedInputTokens: 20,
      cacheCreationTokens: 30,
      outputTokens: 40,
      reasoningTokens: 5,
    },
    costUsd: 1,
    cacheSavingsUsd: 2,
    costSource: "providerReported",
    records: 1,
    unpricedRecords: 0,
    sessions: 1,
    ...overrides,
  };
}

function success(label: string, value: UsageSummary): EnvironmentUsageResult {
  return { environmentId: label, label, state: "success", summary: value };
}

describe("mergeUsageEnvironments", () => {
  it("withholds totals until every non-failed environment has settled", () => {
    const result = mergeUsageEnvironments([
      success("Local", summary({ sources: [source("claude")] })),
      { environmentId: "remote", label: "Remote", state: "pending" },
    ]);

    expect(result.isPending).toBe(true);
    expect(result.summary).toBeNull();
    expect(result.notices).toEqual([]);
  });

  it("treats failures and incompatible versions as terminal, visible coverage", () => {
    const result = mergeUsageEnvironments([
      success("Local", summary({ sources: [source("claude")] })),
      { environmentId: "failed", label: "Offline box", state: "failed", message: "socket closed" },
      success("Old box", summary({ contractVersion: USAGE_CONTRACT_VERSION - 1 })),
    ]);

    expect(result.isPending).toBe(false);
    expect(result.summary).not.toBeNull();
    expect(result.notices.map((notice) => notice.kind)).toEqual(["failed", "incompatible"]);
    expect(result.notices.map((notice) => notice.label)).toEqual(["Offline box", "Old box"]);
  });

  it("deduplicates all-four-field fingerprints while keeping providers and physical hosts distinct", () => {
    const claude = source("claude");
    const result = mergeUsageEnvironments([
      success("First", summary({ sources: [claude], buckets: [bucket("claude", "sonnet")] })),
      success("Duplicate", summary({ sources: [claude], buckets: [bucket("claude", "sonnet")] })),
      success(
        "Other host",
        summary({
          sources: [source("claude", { fingerprint: { ...claude.fingerprint, hostId: "host-b" } })],
          buckets: [bucket("claude", "sonnet")],
        }),
      ),
      success(
        "Other volume",
        summary({
          sources: [source("claude", { fingerprint: { ...claude.fingerprint, volumeId: "8:9" } })],
          buckets: [bucket("claude", "sonnet")],
        }),
      ),
      success(
        "Codex same location",
        summary({
          sources: [
            source("codex", {
              fingerprint: { ...claude.fingerprint, provider: "codex" },
            }),
          ],
          buckets: [bucket("codex", "gpt")],
        }),
      ),
    ]);

    expect(result.summary?.totals.processedTokens).toBe(400);
    expect(result.summary?.distinctSessions).toBe(4);
    expect(result.notices.filter((notice) => notice.kind === "duplicate")).toHaveLength(1);
  });

  it("prefers a healthy duplicate source regardless of environment order", () => {
    const shared = source("claude");
    const unhealthy = success(
      "Unavailable first",
      summary({ sources: [{ ...shared, status: "failed", message: "scan failed" }] }),
    );
    const healthy = success(
      "Healthy second",
      summary({ sources: [shared], buckets: [bucket("claude", "sonnet")] }),
    );

    for (const environments of [
      [unhealthy, healthy],
      [healthy, unhealthy],
    ]) {
      const result = mergeUsageEnvironments(environments);
      expect(result.summary?.totals.processedTokens).toBe(100);
      expect(result.summary?.sources).toEqual([shared]);
      expect(result.notices.some((notice) => notice.kind === "failed")).toBe(true);
      expect(result.notices.some((notice) => notice.kind === "duplicate")).toBe(true);
    }
  });

  it("merges stable day/provider/model buckets and preserves every arithmetic category", () => {
    const result = mergeUsageEnvironments([
      success(
        "A",
        summary({
          sources: [source("claude", { distinctSessions: 2 })],
          buckets: [
            bucket("claude", "sonnet", { day: "2026-08-08" as never, sessions: 2 }),
            bucket("claude", "sonnet", {
              totals: {
                uncachedInputTokens: 1,
                cachedInputTokens: 2,
                cacheCreationTokens: 3,
                outputTokens: 4,
                reasoningTokens: 1,
              },
              costUsd: 3,
              cacheSavingsUsd: 4,
              records: 2,
              sessions: 2,
            }),
          ],
        }),
      ),
      success(
        "B",
        summary({
          sources: [
            source("claude", { fingerprint: { ...source("claude").fingerprint, hostId: "b" } }),
          ],
          buckets: [bucket("claude", "sonnet", { costSource: "modelPriced" })],
        }),
      ),
    ]);

    expect(result.summary?.buckets.map((value) => `${value.day}:${value.model}`)).toEqual([
      "2026-08-08:sonnet",
      "2026-08-09:sonnet",
    ]);
    expect(result.summary?.buckets[1]).toMatchObject({
      totals: {
        uncachedInputTokens: 11,
        cachedInputTokens: 22,
        cacheCreationTokens: 33,
        outputTokens: 44,
        reasoningTokens: 6,
      },
      costUsd: 4,
      cacheSavingsUsd: 6,
      costSource: "modelPriced",
      records: 3,
      sessions: 3,
    });
    // The first source's same two sessions span both days; bucket cardinalities are not summed.
    expect(result.summary?.distinctSessions).toBe(3);
  });

  it("uses weakest pricing and cost provenance and computes provider/model/day shares", () => {
    const result = mergeUsageEnvironments([
      success(
        "Claude",
        summary({
          sources: [source("claude")],
          buckets: [bucket("claude", "sonnet", { costUsd: 3 })],
        }),
      ),
      success(
        "Codex",
        summary({
          pricing: { status: "cached", source: "disk", fetchedAt: "2026-08-08", knownModels: 1 },
          sources: [source("codex")],
          buckets: [
            bucket("codex", "gpt", {
              totals: {
                uncachedInputTokens: 100,
                cachedInputTokens: 200,
                cacheCreationTokens: 300,
                outputTokens: 400,
                reasoningTokens: 50,
              },
              costUsd: 1,
              costSource: "unpriced",
              unpricedRecords: 1,
            }),
          ],
        }),
      ),
    ]);

    expect(result.summary?.pricing.status).toBe("cached");
    expect(result.summary?.buckets.find((value) => value.provider === "codex")?.costSource).toBe(
      "unpriced",
    );
    expect(result.summary?.totals.processedTokens).toBe(1_100);
    expect(result.summary?.byProvider.map(({ provider }) => provider)).toEqual(["claude", "codex"]);
    expect(result.summary?.byProvider[0]?.tokenShare).toBeCloseTo(100 / 1_100);
    expect(result.summary?.byProvider[1]?.costShare).toBeCloseTo(1 / 4);
    expect(result.summary?.byModel.map(({ provider, model }) => `${provider}:${model}`)).toEqual([
      "claude:sonnet",
      "codex:gpt",
    ]);
    expect(result.summary?.byDay).toHaveLength(1);
    expect(result.summary?.byDay[0]?.tokenShare).toBe(1);
  });

  it("reports bounded missing, partial, and source failures without leaking raw content", () => {
    const raw = `secret prompt ${"x".repeat(300)}`;
    const result = mergeUsageEnvironments([
      success(
        "Box",
        summary({
          sources: [
            source("claude", { status: "missing", message: null }),
            source("codex", { status: "partial", message: "Some transcript rows were skipped." }),
            source("codex", {
              fingerprint: { ...source("codex").fingerprint, volumeId: "other" },
              status: "failed",
              message: raw,
            }),
          ],
        }),
      ),
    ]);

    expect(result.notices.map(({ kind }) => kind)).toEqual(["missing", "partial", "failed"]);
    expect(result.notices.every(({ message }) => message.length <= 240)).toBe(true);
    expect(JSON.stringify(result.summary)).not.toContain("secret prompt");
  });
});
