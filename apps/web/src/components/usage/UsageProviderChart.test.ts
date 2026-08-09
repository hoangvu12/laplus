import type { UsageBucket, UsageDay } from "@t3tools/contracts";
import { describe, expect, it } from "vite-plus/test";

import { buildDayColumns, enumerateUsageDays, niceScale } from "./UsageProviderChart";

function bucket(overrides: Partial<UsageBucket>): UsageBucket {
  return {
    day: "2026-08-01" as UsageDay,
    provider: "claude",
    model: "fixture",
    totals: {
      uncachedInputTokens: 10,
      cachedInputTokens: 20,
      cacheCreationTokens: 30,
      outputTokens: 40,
      reasoningTokens: 5,
    },
    costUsd: 2,
    cacheSavingsUsd: 1,
    costSource: "modelPriced",
    records: 1,
    unpricedRecords: 0,
    sessions: 1,
    ...overrides,
  };
}

describe("usage chart calendar", () => {
  it("enumerates the inclusive range, including empty days", () => {
    expect(enumerateUsageDays("2026-08-01", "2026-08-03")).toEqual([
      "2026-08-01",
      "2026-08-02",
      "2026-08-03",
    ]);
  });
});

describe("niceScale", () => {
  it("uses an even 1/2/5 scale that never clips the peak", () => {
    for (const peak of [0.04, 1, 37.5, 999, 1122.71, 5_000, 1_400_000_000]) {
      const { max, ticks } = niceScale(peak, 4);
      expect(max).toBeGreaterThanOrEqual(peak);
      expect(ticks[0]).toBe(0);
      expect(ticks.at(-1)).toBe(max);
      const steps = ticks.slice(1).map((tick, index) => tick - (ticks[index] ?? 0));
      for (const step of steps) expect(step).toBeCloseTo(steps[0] ?? 0, 8);
    }
  });

  it("degrades to a zero tick for an empty series", () => {
    expect(niceScale(0, 4)).toEqual({ max: 0, ticks: [0] });
  });
});

describe("buildDayColumns", () => {
  const days = ["2026-08-01", "2026-08-02", "2026-08-03"];
  const buckets = [
    bucket({ provider: "codex", costUsd: 1 }),
    bucket({ provider: "claude", costUsd: 2 }),
    bucket({ day: "2026-08-03" as UsageDay, provider: "claude", costUsd: 5 }),
  ];

  it("keeps an explicit zero column for days with no activity", () => {
    expect(buildDayColumns(days, buckets, "cost").map(({ total }) => total)).toEqual([3, 0, 5]);
  });

  it("reads processed tokens without adding reasoning twice", () => {
    expect(buildDayColumns(days, buckets, "tokens").map(({ total }) => total)).toEqual([
      200, 0, 100,
    ]);
  });

  it("keeps provider bands absolute rather than cumulative", () => {
    expect(buildDayColumns(days, buckets, "cost")[0]?.bands).toEqual([
      { provider: "codex", value: 1 },
      { provider: "claude", value: 2 },
    ]);
  });
});
