import * as Schema from "effect/Schema";
import { describe, expect, it } from "vite-plus/test";

import { UsageReadError, UsageSummary } from "./usage.ts";

const decodeUsageSummary = Schema.decodeUnknownSync(UsageSummary);

const validSummary = {
  contractVersion: 3,
  readAt: "2026-08-09T12:00:00Z",
  timeZone: "America/New_York",
  sinceDay: "2026-08-03",
  untilDay: "2026-08-09",
  buckets: [
    {
      day: "2026-08-09",
      provider: "claude",
      model: "claude-sonnet-4-5",
      totals: {
        uncachedInputTokens: 11,
        cachedInputTokens: 12,
        cacheCreationTokens: 13,
        outputTokens: 14,
        reasoningTokens: 0,
      },
      costUsd: 0,
      cacheSavingsUsd: 0,
      costSource: "unpriced",
      records: 1,
      unpricedRecords: 1,
      sessions: 1,
    },
  ],
  sources: [
    {
      fingerprint: {
        hostId: "host",
        provider: "claude",
        resolvedHomePath: "/tmp/.claude",
        volumeId: "1:2",
      },
      status: "ok",
      scannedFiles: 1,
      skippedFiles: 0,
      malformedRecords: 0,
      distinctSessions: 1,
      message: null,
    },
  ],
  pricing: { status: "unavailable", source: "none", fetchedAt: null, knownModels: 0 },
  scanDurationMs: 2,
};

describe("Usage reporting contract", () => {
  it("decodes a versioned aggregate without transcript content", () => {
    const decoded = decodeUsageSummary(validSummary);
    expect(decoded.buckets[0]?.totals.outputTokens).toBe(14);
    expect(JSON.stringify(decoded)).not.toContain("prompt");
  });

  it.each(["scanFailed", "invalidWindow"] as const)("declares %s as a read failure", (reason) => {
    expect(new UsageReadError({ reason, detail: "bounded detail" }).reason).toBe(reason);
  });

  it("rejects non-calendar day strings", () => {
    expect(() => decodeUsageSummary({ ...validSummary, sinceDay: "August 3" })).toThrow();
  });
});
