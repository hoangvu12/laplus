/**
 * Where a child's work stream is scrolled to, and whether it is still following.
 *
 * Two things live here, and neither of them touches React or the DOM:
 *
 * - **The follow decision.** "Follow live entries only while pinned to the
 *   bottom" is one comparison against three numbers. Kept as a function of
 *   those numbers, it is provable in a unit test; kept inside a scroll handler,
 *   it would only be provable by driving a browser, because neither happy-dom
 *   nor jsdom lays anything out. The scroll *positions* still need a browser —
 *   ticket 07 owns that — but the rule applied to them does not.
 * - **The memory.** Only the active right-panel surface is mounted, so
 *   switching to a file tab and back unmounts and remounts a child's view. Its
 *   scroll position therefore cannot live in that view: it lives here, keyed by
 *   surface, so every child tab keeps its own place across the switch.
 *
 * The memory is session state and is deliberately *not* persisted: after a
 * reload a restored subagent tab replays lazily and opens at its live edge,
 * which is where a reader who has just returned to a running child wants to be.
 */
import { scopedThreadKey } from "@t3tools/client-runtime/environment";
import type { ScopedThreadRef } from "@t3tools/contracts";

/** The three numbers a scroll container reports, and all this module needs. */
export interface ScrollMetrics {
  readonly scrollTop: number;
  readonly scrollHeight: number;
  readonly clientHeight: number;
}

/**
 * How far short of the bottom still counts as being at it.
 *
 * Fractional device pixels, a scrollbar's rounding and a growing last entry all
 * leave a viewport a pixel or two shy of its own content, and a reader watching
 * live output has not "scrolled up" by any of them.
 */
export const PINNED_TO_BOTTOM_SLACK_PX = 24;

export function distanceFromBottom(metrics: ScrollMetrics): number {
  return Math.max(0, metrics.scrollHeight - metrics.clientHeight - metrics.scrollTop);
}

export function isPinnedToBottom(
  metrics: ScrollMetrics,
  slack: number = PINNED_TO_BOTTOM_SLACK_PX,
): boolean {
  return distanceFromBottom(metrics) <= slack;
}

/** Where one child tab was left, and whether it was still watching. */
export interface SubagentScrollPosition {
  readonly offset: number;
  readonly following: boolean;
}

/**
 * The address of one subagent surface's scroll state.
 *
 * The same child id in another thread is another surface, so the thread is part
 * of the key — the right-panel workspace is thread-scoped and its tabs are too.
 * The thread half is `scopedThreadKey`, the same function `rightPanelStore` keys
 * that workspace by, rather than a second spelling of it here: one place decides
 * what "this thread's workspace" is called. Appending the child after a fixed
 * thread prefix cannot collide, whatever a child id contains.
 */
export function subagentScrollKey(ref: ScopedThreadRef, childId: string): string {
  return `${scopedThreadKey(ref)}:${childId}`;
}

/**
 * How many surfaces are remembered before the least recently touched is
 * dropped. Generous enough that no plausible workspace evicts a tab it still
 * has open, small enough that this is not an unbounded map.
 */
const REMEMBERED_SURFACE_LIMIT = 64;

const positions = new Map<string, SubagentScrollPosition>();

export function rememberSubagentScroll(key: string, position: SubagentScrollPosition): void {
  positions.delete(key);
  positions.set(key, position);
  while (positions.size > REMEMBERED_SURFACE_LIMIT) {
    const oldest = positions.keys().next();
    if (oldest.done) break;
    positions.delete(oldest.value);
  }
}

export function readSubagentScroll(key: string): SubagentScrollPosition | null {
  return positions.get(key) ?? null;
}

/**
 * Drop a surface's place, because that surface is no longer open.
 *
 * The spec preserves scroll state "per open child surface", and closing a tab
 * removes the surface. Without this, closing a suspended live child and
 * reopening it from its inline row would drop the reader back at a stale offset
 * with following still suspended — a tab that looks reopened but is showing
 * where they were, not what the child has said since.
 */
export function forgetSubagentScroll(key: string): void {
  positions.delete(key);
}

/** Test seam: the memory outlives any one component, so a test must clear it. */
export function resetSubagentScrollMemoryForTests(): void {
  positions.clear();
}
