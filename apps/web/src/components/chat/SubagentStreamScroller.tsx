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
import { ChevronDownIcon } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import { ScrollArea } from "~/components/ui/scroll-area";

import { isPinnedToBottom, readSubagentScroll, rememberSubagentScroll } from "./subagentScroll";

interface SubagentStreamScrollerProps {
  /** This child surface's address, from `subagentScrollKey`. */
  readonly surfaceKey: string;
  /**
   * Changes whenever the stream has grown or an entry has been extended. The
   * shell does not look at what changed — only that something did, so that a
   * following reader is carried to the new live edge.
   */
  readonly contentKey: string;
  /** The child's lifecycle state, reported the way the panel already reports it. */
  readonly streamState?: string | undefined;
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

  // Following, as the stream grows. A suspended reader is left exactly where
  // they are; the affordance below is how they choose to catch up.
  useLayoutEffect(() => {
    if (!(readSubagentScroll(surfaceKey)?.following ?? true)) return;
    scrollToLiveEdge();
  }, [props.contentKey, scrollToLiveEdge, surfaceKey]);

  return (
    <div
      className="relative flex min-h-0 flex-1 flex-col"
      data-subagent-state={props.streamState}
      data-subagent-following={following ? "true" : "false"}
    >
      <ScrollArea className="min-h-0 flex-1" viewportRef={viewportRef}>
        {props.children}
      </ScrollArea>
      {following ? null : (
        <div className="pointer-events-none absolute inset-x-0 bottom-0 z-10 flex justify-center py-2">
          <button
            type="button"
            aria-label="Scroll to end"
            title="Scroll to end"
            onClick={scrollToLiveEdge}
            className="pointer-events-auto flex items-center gap-1.5 rounded-full border border-border/60 bg-card px-3 py-1 text-muted-foreground text-xs shadow-sm transition-colors hover:cursor-pointer hover:border-border hover:text-foreground"
          >
            <ChevronDownIcon className="size-3.5" />
            Scroll to end
          </button>
        </div>
      )}
    </div>
  );
}

export default SubagentStreamScroller;
