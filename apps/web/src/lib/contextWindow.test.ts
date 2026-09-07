import { describe, expect, it } from "vite-plus/test";
import { EventId, type OrchestrationThreadActivity, TurnId } from "@t3tools/contracts";

import { deriveLatestContextWindowSnapshot, formatContextWindowTokens } from "./contextWindow";

function makeActivity(id: string, kind: string, payload: unknown): OrchestrationThreadActivity {
  return {
    id: EventId.make(id),
    tone: "info",
    kind,
    summary: kind,
    payload,
    turnId: TurnId.make("turn-1"),
    createdAt: "2026-03-23T00:00:00.000Z",
  };
}

describe("contextWindow", () => {
  it("derives the latest valid context window snapshot", () => {
    const snapshot = deriveLatestContextWindowSnapshot([
      makeActivity("activity-1", "context-window.updated", {
        usedTokens: 1000,
      }),
      makeActivity("activity-2", "tool.started", {}),
      makeActivity("activity-3", "context-window.updated", {
        usedTokens: 14_000,
        maxTokens: 258_000,
        compactsAutomatically: true,
      }),
    ]);

    expect(snapshot).not.toBeNull();
    expect(snapshot?.usedTokens).toBe(14_000);
    expect(snapshot?.totalProcessedTokens).toBeNull();
    expect(snapshot?.maxTokens).toBe(258_000);
    expect(snapshot?.compactsAutomatically).toBe(true);
  });

  it("ignores malformed payloads", () => {
    const snapshot = deriveLatestContextWindowSnapshot([
      makeActivity("activity-1", "context-window.updated", {}),
    ]);

    expect(snapshot).toBeNull();
  });

  it("keeps valid zero-usage snapshots", () => {
    const snapshot = deriveLatestContextWindowSnapshot([
      makeActivity("activity-1", "context-window.updated", {
        usedTokens: 0,
        maxTokens: 100_000,
      }),
    ]);

    expect(snapshot).toMatchObject({
      usedTokens: 0,
      maxTokens: 100_000,
      remainingTokens: 100_000,
      usedPercentage: 0,
      remainingPercentage: 100,
    });
  });

  it.each([0, 12_000])(
    "reads persisted Mimir occupancy of %s tokens without inventing usage totals",
    (usedTokens) => {
      const snapshot = deriveLatestContextWindowSnapshot([
        makeActivity("mimir-context-1", "context.usage", {
          usedTokens,
          maxTokens: 128_000,
          detail: "Mimir context occupancy (not billable usage or cost).",
        }),
        makeActivity("mimir-usage-1", "tokens.usage", {
          usage: { input_tokens: 900_000, output_tokens: 10_000 },
          detail: "Mimir token counts; monetary cost is unavailable.",
        }),
      ]);

      expect(snapshot).toMatchObject({
        usedTokens,
        maxTokens: 128_000,
        remainingTokens: 128_000 - usedTokens,
        usedPercentage: (usedTokens / 128_000) * 100,
        totalProcessedTokens: null,
        inputTokens: null,
        outputTokens: null,
        compactsAutomatically: false,
      });
    },
  );

  it.each(["context-window.updated", "context.usage"])(
    "uses the latest valid %s reading in mixed native and persisted Mimir history",
    (latestKind) => {
      const olderKind = latestKind === "context.usage" ? "context-window.updated" : "context.usage";
      const snapshot = deriveLatestContextWindowSnapshot([
        makeActivity("older", olderKind, { usedTokens: 1_000, maxTokens: 128_000 }),
        makeActivity("newer", latestKind, { usedTokens: 2_000, maxTokens: 128_000 }),
        makeActivity("cumulative", "tokens.usage", { usage: { input_tokens: 900_000 } }),
        makeActivity("invalid", "context.usage", { usedTokens: -1, maxTokens: 128_000 }),
      ]);

      expect(snapshot?.usedTokens).toBe(2_000);
      expect(snapshot?.totalProcessedTokens).toBeNull();
    },
  );

  it("does not let malformed legacy rows replace a complete canonical reading", () => {
    const snapshot = deriveLatestContextWindowSnapshot([
      makeActivity("native", "context-window.updated", {
        usedTokens: 4_000,
        maxTokens: 128_000,
        totalProcessedTokens: 240_000,
        compactsAutomatically: true,
      }),
      makeActivity("missing", "context.usage", {}),
      makeActivity("invalid", "context.usage", { usedTokens: "5000" }),
      makeActivity("not-finite", "context.usage", { usedTokens: Infinity }),
    ]);

    expect(snapshot).toMatchObject({
      usedTokens: 4_000,
      totalProcessedTokens: 240_000,
      compactsAutomatically: true,
    });
    expect(
      deriveLatestContextWindowSnapshot([
        makeActivity("tokens-only", "tokens.usage", { usage: { input_tokens: 900_000 } }),
      ]),
    ).toBeNull();
  });

  it("formats compact token counts", () => {
    expect(formatContextWindowTokens(999)).toBe("999");
    expect(formatContextWindowTokens(1400)).toBe("1.4k");
    expect(formatContextWindowTokens(14_000)).toBe("14k");
    expect(formatContextWindowTokens(258_000)).toBe("258k");
  });

  it("includes total processed tokens when available", () => {
    const snapshot = deriveLatestContextWindowSnapshot([
      makeActivity("activity-1", "context-window.updated", {
        usedTokens: 81_659,
        totalProcessedTokens: 748_126,
        maxTokens: 258_400,
        lastUsedTokens: 81_659,
      }),
    ]);

    expect(snapshot?.usedTokens).toBe(81_659);
    expect(snapshot?.totalProcessedTokens).toBe(748_126);
  });
});
