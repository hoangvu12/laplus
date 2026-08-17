/**
 * The scrolling shell a child's work stream is read through.
 *
 * Deliberately a shell and nothing else: it owns where the reader is, whether
 * they are still watching the live edge, and the way back to it. What is inside
 * it is `SubagentStreamPanel`'s business, and the two are separate files
 * because they change for different reasons — a new kind of child entry is a
 * change to the stream, not to the way one is read.
 *
 * Two behaviours it exists for, both of which the workspace already implies:
 *
 * - **Following is pinned-to-bottom, not always-on.** A child that is still
 *   working keeps producing entries; a reader at the bottom is watching it and
 *   a reader who has scrolled up is reading something. Scrolling up therefore
 *   suspends following, and the same "Scroll to end" affordance the main
 *   transcript uses is how the reader chooses to come back.
 * - **A tab keeps its place.** Only the active right-panel surface is mounted,
 *   so switching to a file tab and back unmounts and remounts this. The place
 *   is kept in `subagentScroll`'s memory, per surface, which is why it survives
 *   a switch and why two child tabs never share one.
 */
import type { OrchestrationSubagentState } from "@t3tools/contracts";
import { type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import { ScrollArea } from "~/components/ui/scroll-area";

import { ScrollToEndButton } from "./ScrollToEndButton";
import { isPinnedToBottom, readSubagentScroll, rememberSubagentScroll } from "./subagentScroll";

interface SubagentStreamScrollerProps {
  /** This subagent surface's address, from `subagentScrollKey`. */
  readonly surfaceKey: string;
  /** The child's lifecycle state, reported the way the panel already reports it. */
  readonly streamState?: OrchestrationSubagentState | undefined;
  readonly children: ReactNode;
}

function liveEdgeOffset(viewport: HTMLElement): number {
  return Math.max(0, viewport.scrollHeight - viewport.clientHeight);
}

export function SubagentStreamScroller(props: SubagentStreamScrollerProps) {
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const surfaceKey = props.surfaceKey;
  /**
   * Whether this surface is following is the memory's answer, not React's: the
   * memory is what survives the unmount a tab switch causes, and keeping one
   * answer avoids the two disagreeing. The state below exists to *draw* the
   * affordance, and is written wherever the memory is.
   *
   * Not `useSyncExternalStore`, which is how `useNowMinute` reads module state:
   * that earns a subscription because many consumers must agree about one
   * value. Here the right panel mounts only its active surface, so a key has at
   * most one reader at a time and the two copies cannot disagree. A subscription
   * would be machinery for a race that the workspace's own structure rules out.
   */
  const isFollowing = () => readSubagentScroll(surfaceKey)?.following ?? true;
  const [following, setFollowing] = useState(isFollowing);

  const scrollToLiveEdge = useCallback(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    viewport.scrollTop = liveEdgeOffset(viewport);
    // Said rather than inferred from the scroll event this provokes: the reader
    // asked to follow again, and that is true before the browser reports it.
    setFollowing(true);
    rememberSubagentScroll(surfaceKey, { offset: viewport.scrollTop, following: true });
  }, [surfaceKey]);

  // Restoring, on the way in. A child opened for the first time has no
  // remembered place and starts where a reader who just clicked its row wants
  // to be: at whatever it has said most recently.
  useLayoutEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const remembered = readSubagentScroll(surfaceKey);
    viewport.scrollTop = remembered ? remembered.offset : liveEdgeOffset(viewport);
    setFollowing(remembered?.following ?? true);
  }, [surfaceKey]);

  useEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const onScroll = () => {
      const pinned = isPinnedToBottom(viewport);
      rememberSubagentScroll(surfaceKey, { offset: viewport.scrollTop, following: pinned });
      setFollowing(pinned);
    };
    viewport.addEventListener("scroll", onScroll, { passive: true });
    return () => viewport.removeEventListener("scroll", onScroll);
  }, [surfaceKey]);

  /**
   * Following, as the stream grows. A suspended reader is left exactly where
   * they are; the affordance below is how they choose to catch up.
   *
   * **No dependency array, deliberately.** The obvious alternative is a key
   * describing the content — entry count, last entry id, the stream's
   * `updatedAt` — and every version of that key is wrong here. A child's
   * entries are *upserted by id* (`applySubagentStreamItem`), so a command
   * going from running to finished, or a blocker being resolved, grows an
   * entry that already exists: the count does not move, the last id does not
   * move, and `updatedAt` arrives in a **separate** `stream-updated` message
   * from the `entry-upserted` that carried the growth — one commit later, and
   * at millisecond resolution, so two writes in one millisecond do not move it
   * at all. A shell that has to be told what changed can be lied to by a
   * payload it does not own. Re-pinning on every commit cannot: it asks the
   * viewport where the bottom is now, and assigning the offset it already has
   * is a no-op in the browser.
   */
  useLayoutEffect(() => {
    if (!(readSubagentScroll(surfaceKey)?.following ?? true)) return;
    scrollToLiveEdge();
  });

  return (
    <div className="relative flex min-h-0 flex-1 flex-col" data-subagent-state={props.streamState}>
      <ScrollArea className="min-h-0 flex-1" viewportRef={viewportRef}>
        {props.children}
      </ScrollArea>
      {following ? null : (
        <div className="pointer-events-none absolute inset-x-0 bottom-0 z-10 flex justify-center py-2">
          <ScrollToEndButton onClick={scrollToLiveEdge} />
        </div>
      )}
    </div>
  );
}

export default SubagentStreamScroller;
