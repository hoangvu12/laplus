import { describe, expect, it } from "vite-plus/test";
import {
  capableDestinationIndex,
  fractionalKeyBetween,
  planPinnedReorder,
  sortPinnedThreads,
  spreadFractionalKeys,
  shouldReleaseOptimisticPinOrder,
} from "./pinnedThreadOrdering";

const pin = (
  environmentId: string,
  id: string,
  pinOrderKey: string | null,
  createdAt = "2026-01-01T00:00:00Z",
) => ({
  environmentId,
  id,
  pinOrderKey,
  pinnedAt: "2026-01-02T00:00:00Z",
  createdAt,
});

describe("pinned thread ordering", () => {
  it("sorts keyed pins first and resolves equal keys by scoped identity", () => {
    const sorted = sortPinnedThreads([
      pin("z", "same", "m"),
      pin("a", "same", null, "2026-04-01T00:00:00Z"),
      pin("a", "same", "m"),
      pin("a", "older", null, "2026-03-01T00:00:00Z"),
    ]);
    expect(sorted.map(({ environmentId, id }) => `${environmentId}:${id}`)).toEqual([
      "a:same",
      "z:same",
      "a:same",
      "a:older",
    ]);
  });

  it("preserves canonical order when project scope selects a subsequence", () => {
    const all = sortPinnedThreads([
      { ...pin("e", "a", "f"), project: "one" },
      { ...pin("e", "b", "m"), project: "two" },
      { ...pin("e", "c", "t"), project: "one" },
    ]);
    expect(all.filter((thread) => thread.project === "one").map((thread) => thread.id)).toEqual([
      "a",
      "c",
    ]);
  });

  it("creates keys between ordinary bounds and detects adjacent or invalid bounds", () => {
    expect(fractionalKeyBetween("b", "d")).toBe("c");
    expect(fractionalKeyBetween("m", null)).toBe("mn");
    expect(fractionalKeyBetween(null, "c")).toBe("b");
    expect(fractionalKeyBetween("a", "aa")).toBeNull();
    expect(fractionalKeyBetween("A", "c")).toBeNull();
    expect(fractionalKeyBetween("g", "h")).toBe("gn");
    expect(fractionalKeyBetween("ga", "h")).toBeNull();
  });

  it("keeps producing valid keys under deep insertion churn", () => {
    let lower = "g";
    for (let index = 0; index < 100; index += 1) {
      const next = fractionalKeyBetween(lower, "h");
      expect(next).not.toBeNull();
      expect(next! > lower && next! < "h").toBe(true);
      expect(next!.endsWith("a")).toBe(false);
      lower = next!;
    }
  });

  it("spreads materialized keys evenly and in lexical order", () => {
    expect(spreadFractionalKeys(4)).toEqual(["f", "k", "p", "u"]);
  });

  it("plans a normal move as one assignment", () => {
    const pins = [pin("e1", "one", "f"), pin("e2", "two", "k"), pin("e1", "three", "p")];
    expect(planPinnedReorder(pins, { environmentId: "e1", id: "three" }, 1)).toEqual([
      { environmentId: "e1", threadId: "three", pinOrderKey: "h" },
    ]);
  });

  it("materializes the canonical section when a keyless boundary prevents insertion", () => {
    const pins = [pin("e1", "one", "f"), pin("e2", "legacy", null), pin("e1", "three", null)];
    const plan = planPinnedReorder(pins, { environmentId: "e1", id: "one" }, 2);
    expect(plan).toEqual([
      { environmentId: "e1", threadId: "three", pinOrderKey: "g" },
      { environmentId: "e2", threadId: "legacy", pinOrderKey: "n" },
      { environmentId: "e1", threadId: "one", pinOrderKey: "t" },
    ]);
  });

  it("never assigns keys to pins owned by legacy environments", () => {
    const pins = [
      { ...pin("modern", "one", "f"), reorderCapable: true },
      { ...pin("legacy", "old", null), reorderCapable: false },
      { ...pin("modern", "two", null), reorderCapable: true },
    ];
    expect(
      planPinnedReorder(pins, { environmentId: "modern", id: "one" }, 1).every(
        (assignment) => assignment.environmentId === "modern",
      ),
    ).toBe(true);
  });

  it("keeps legacy pins as fixed boundaries when reordering capable pins", () => {
    const pins = [
      { ...pin("modern", "one", "f"), reorderCapable: true },
      { ...pin("legacy", "old", "m"), reorderCapable: false },
      { ...pin("modern", "two", "t"), reorderCapable: true },
    ];

    const plan = planPinnedReorder(pins, { environmentId: "modern", id: "two" }, 0);
    const assignments = new Map(
      plan.map((assignment) => [
        `${assignment.environmentId}:${assignment.threadId}`,
        assignment.pinOrderKey,
      ]),
    );
    const persisted = sortPinnedThreads(
      pins.map((thread) => ({
        ...thread,
        pinOrderKey: assignments.get(`${thread.environmentId}:${thread.id}`) ?? thread.pinOrderKey,
      })),
    );

    expect(plan).toHaveLength(2);
    expect(plan.every((assignment) => assignment.environmentId === "modern")).toBe(true);
    expect(persisted.map((thread) => thread.id)).toEqual(["two", "old", "one"]);
  });

  it("maps a drop on a legacy row to the adjacent capable slot", () => {
    const pins = [
      { ...pin("modern", "one", "f"), reorderCapable: true },
      { ...pin("legacy", "old", "m"), reorderCapable: false },
      { ...pin("modern", "two", "t"), reorderCapable: true },
    ];

    expect(
      capableDestinationIndex(
        pins,
        "modern:two",
        "legacy:old",
        (thread) => `${thread.environmentId}:${thread.id}`,
        (thread) => thread.reorderCapable !== false,
      ),
    ).toBe(0);
    expect(
      capableDestinationIndex(
        pins,
        "modern:one",
        "legacy:old",
        (thread) => `${thread.environmentId}:${thread.id}`,
        (thread) => thread.reorderCapable !== false,
      ),
    ).toBe(1);
  });

  it("holds partial own writes and releases on confirmation, membership, or a foreign key", () => {
    const keysAtDrop = new Map([
      ["a", "f"],
      ["b", "m"],
    ]);
    const expected = new Map([
      ["a", "g"],
      ["b", "n"],
    ]);
    expect(
      shouldReleaseOptimisticPinOrder({
        keysAtDrop,
        expected,
        current: new Map([
          ["a", "g"],
          ["b", "m"],
        ]),
      }),
    ).toBe(false);
    expect(
      shouldReleaseOptimisticPinOrder({
        keysAtDrop,
        expected,
        current: new Map([
          ["a", "g"],
          ["b", "n"],
        ]),
      }),
    ).toBe(true);
    expect(
      shouldReleaseOptimisticPinOrder({
        keysAtDrop,
        expected,
        current: new Map([
          ["a", "z"],
          ["b", "m"],
        ]),
      }),
    ).toBe(true);
    expect(
      shouldReleaseOptimisticPinOrder({ keysAtDrop, expected, current: new Map([["a", "f"]]) }),
    ).toBe(true);
    expect(
      shouldReleaseOptimisticPinOrder({ keysAtDrop, expected, current: keysAtDrop, failed: true }),
    ).toBe(true);
  });
});
