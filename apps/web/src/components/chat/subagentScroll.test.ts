/**
 * The follow/suspend decision and the per-surface scroll memory, as units.
 *
 * These are the parts of "a live child follows only while pinned to the bottom"
 * that do not need a browser. What is deliberately *not* asserted here is
 * anything about real layout: no test in this repository can prove that a
 * viewport's metrics move the way a browser moves them, because neither
 * happy-dom nor jsdom lays anything out. Ticket 07's browser-driver scenario
 * owns that half; this file owns the decision made from whatever metrics
 * arrive.
 */
import { scopeThreadRef, scopedThreadKey } from "@t3tools/client-runtime/environment";
import { type EnvironmentId, ThreadId } from "@t3tools/contracts";
import { describe, expect, it } from "vite-plus/test";

import {
  PINNED_TO_BOTTOM_SLACK_PX,
  distanceFromBottom,
  forgetSubagentScroll,
  isPinnedToBottom,
  readSubagentScroll,
  rememberSubagentScroll,
  resetSubagentScrollMemoryForTests,
  subagentScrollKey,
} from "./subagentScroll";

const metrics = (scrollTop: number, scrollHeight = 1000, clientHeight = 400) => ({
  scrollTop,
  scrollHeight,
  clientHeight,
});

const threadRef = (threadId: string) =>
  scopeThreadRef("env-1" as EnvironmentId, ThreadId.make(threadId));

describe("deciding whether a child stream is pinned to its live edge", () => {
  it("is pinned when the viewport bottom has reached the content bottom", () => {
    expect(isPinnedToBottom(metrics(600))).toBe(true);
    expect(distanceFromBottom(metrics(600))).toBe(0);
  });

  it("is still pinned within the slack a sub-pixel viewport leaves behind", () => {
    expect(PINNED_TO_BOTTOM_SLACK_PX).toBeGreaterThan(0);
    expect(isPinnedToBottom(metrics(600 - PINNED_TO_BOTTOM_SLACK_PX))).toBe(true);
  });

  it("is not pinned once the reader has scrolled further up than that", () => {
    expect(isPinnedToBottom(metrics(600 - PINNED_TO_BOTTOM_SLACK_PX - 1))).toBe(false);
    expect(isPinnedToBottom(metrics(0))).toBe(false);
    expect(distanceFromBottom(metrics(0))).toBe(600);
  });

  /**
   * A stream shorter than its viewport has no live edge to fall away from, so
   * a child that has said one thing follows the next one.
   */
  it("counts content that does not overflow as pinned", () => {
    expect(isPinnedToBottom(metrics(0, 120, 400))).toBe(true);
  });

  /** Over-scroll — rubber banding, or a shrinking stream — is still the edge. */
  it("counts an over-scrolled viewport as pinned", () => {
    expect(isPinnedToBottom(metrics(900))).toBe(true);
  });
});

describe("remembering where each child tab was left", () => {
  it("keeps one position per surface and hands each back independently", () => {
    resetSubagentScrollMemoryForTests();
    rememberSubagentScroll("env-1:thread-A:child-1", { offset: 240, following: false });
    rememberSubagentScroll("env-1:thread-A:child-2", { offset: 0, following: true });

    expect(readSubagentScroll("env-1:thread-A:child-1")).toEqual({
      offset: 240,
      following: false,
    });
    expect(readSubagentScroll("env-1:thread-A:child-2")).toEqual({ offset: 0, following: true });
  });

  it("knows nothing about a child that has not been read yet", () => {
    resetSubagentScrollMemoryForTests();
    expect(readSubagentScroll("env-1:thread-A:child-9")).toBeNull();
  });

  /**
   * The same child in two threads is two surfaces, and two positions. The
   * thread half of the key is `scopedThreadKey` — the same function the
   * right-panel store keys that workspace by — so a surface cannot be addressed
   * one way here and another way there.
   */
  it("scopes a position to its thread as well as its child", () => {
    resetSubagentScrollMemoryForTests();
    const inThreadA = subagentScrollKey(threadRef("thread-A"), "child-1");
    const inThreadB = subagentScrollKey(threadRef("thread-B"), "child-1");
    expect(inThreadA).not.toBe(inThreadB);
    expect(inThreadA.startsWith(scopedThreadKey(threadRef("thread-A")))).toBe(true);

    rememberSubagentScroll(inThreadA, { offset: 100, following: false });
    expect(readSubagentScroll(inThreadB)).toBeNull();
  });

  /** Two children of one thread are two surfaces, whatever their ids contain. */
  it("cannot confuse two children of the same thread", () => {
    resetSubagentScrollMemoryForTests();
    const ref = threadRef("thread-A");
    expect(subagentScrollKey(ref, "call:1")).not.toBe(subagentScrollKey(ref, "call:2"));

    rememberSubagentScroll(subagentScrollKey(ref, "call:1"), { offset: 40, following: false });
    expect(readSubagentScroll(subagentScrollKey(ref, "call:2"))).toBeNull();
  });

  /**
   * A closed tab is not an open surface, so its place goes with it. Closing a
   * suspended live child and reopening it from its inline row must show what
   * the child has said since, not where the reader was before.
   * `rightPanelCleanup.test.ts` proves closing asks for this.
   */
  it("forgets a surface that is no longer open, and starts it at the live edge again", () => {
    resetSubagentScrollMemoryForTests();
    rememberSubagentScroll("env-1:thread-A:child-1", { offset: 240, following: false });
    rememberSubagentScroll("env-1:thread-A:child-2", { offset: 90, following: false });

    forgetSubagentScroll("env-1:thread-A:child-1");

    expect(readSubagentScroll("env-1:thread-A:child-1")).toBeNull();
    // And only that one: closing a tab is not closing the workspace.
    expect(readSubagentScroll("env-1:thread-A:child-2")).toEqual({ offset: 90, following: false });
  });

  /**
   * Session state, not a leak: the memory is bounded, and the surface evicted
   * is the one left longest ago rather than the one being read.
   */
  it("bounds itself by dropping the least recently touched surface", () => {
    resetSubagentScrollMemoryForTests();
    const keys = Array.from({ length: 80 }, (_, index) => `env-1:thread-A:child-${index}`);
    for (const key of keys) rememberSubagentScroll(key, { offset: 1, following: false });

    expect(readSubagentScroll(keys[0]!)).toBeNull();
    expect(readSubagentScroll(keys.at(-1)!)).toEqual({ offset: 1, following: false });
  });
});
