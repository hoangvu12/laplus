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
 * reload a restored child tab replays lazily and opens at its live edge, which
 * is where a reader who has just returned to a running child wants to be.
 */

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
 * The address of one child surface's scroll state.
 *
 * The same child id in another thread is another surface, so the thread is part
 * of the key — the right-panel workspace is thread-scoped and its tabs are too.
 */
export function subagentScrollKey(
  environmentId: string,
  threadId: string,
  childId: string,
): string {
  return `${environmentId}:${threadId}:${childId}`;
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

export function forgetSubagentScroll(key: string): void {
  positions.delete(key);
}

/** Test seam: the memory outlives any one component, so a test must clear it. */
export function resetSubagentScrollMemory(): void {
  positions.clear();
}
