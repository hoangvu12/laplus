/**
 * What closing a right-panel surface entails beyond removing the tab.
 *
 * A right-panel tab is presentation, but two of the surface kinds are windows
 * onto something the server owns: a browser tab is a preview session and a
 * terminal tab is one or more shells. Closing those tabs must stop those
 * things; closing any other tab must not reach the server at all.
 *
 * The distinction used to live inside a `useCallback` in `ChatView`, where the
 * only way to find out whether closing a *subagent* tab could interrupt the
 * child was to read a loop and check that no branch matched. Naming the answer
 * makes it a value a test can hold.
 *
 * **The split between the two groups below is the point.** `dismissPlanSidebar`,
 * `previewTabIds` and `terminalIds` are the complete set of calls a close may
 * make that leave this window; `forgottenSubagentChildIds` reaches nothing at
 * all. So "closing a subagent tab emits no interrupt, cancellation, detachment
 * or provider command" is not an argument about the code — it is the shape of
 * this value, and a test can state it.
 */
import type { RightPanelSurface } from "./rightPanelStore";

export interface RightPanelSurfaceCleanup {
  /** The plan sidebar is dismissed for the current turn with its surface. */
  readonly dismissPlanSidebar: boolean;
  /** Preview sessions to close, in the order their tabs appeared. */
  readonly previewTabIds: readonly string[];
  /** Terminal sessions to close, including every pane of a split surface. */
  readonly terminalIds: readonly string[];
  /**
   * Subagent surfaces whose scroll and follow state should be released.
   *
   * Local to this window, and the whole of what closing a subagent tab does:
   * the server goes on recording the child either way. Kept apart from the
   * three fields above so that "closing a child asks the server for nothing"
   * stays a claim about this value rather than about the reader's memory.
   */
  readonly forgottenSubagentChildIds: readonly string[];
}

/** True when closing these surfaces would reach outside this window. */
export function cleanupReachesTheServer(cleanup: RightPanelSurfaceCleanup): boolean {
  return (
    cleanup.dismissPlanSidebar || cleanup.previewTabIds.length > 0 || cleanup.terminalIds.length > 0
  );
}

export function resolveRightPanelSurfaceCleanup(
  surfaces: readonly RightPanelSurface[],
): RightPanelSurfaceCleanup {
  const previewTabIds: string[] = [];
  const terminalIds: string[] = [];
  const forgottenSubagentChildIds: string[] = [];
  let dismissPlanSidebar = false;

  for (const surface of surfaces) {
    switch (surface.kind) {
      case "plan":
        dismissPlanSidebar = true;
        break;
      case "preview":
        if (surface.resourceId) previewTabIds.push(surface.resourceId);
        break;
      case "terminal":
        terminalIds.push(...surface.terminalIds);
        break;
      // A subagent work stream is recorded by the server whether or not anyone
      // is watching it, so hiding the view releases nothing and stops nothing.
      // What it does release is the reader's place in it, because that place
      // belongs to an open surface and this one is closing.
      case "subagent":
        forgottenSubagentChildIds.push(surface.resourceId);
        break;
      // Files, the explorer and the diff are free: nothing is held for them.
      case "file":
      case "files":
      case "diff":
        break;
      default:
        // Unreachable — but a kind added to `RIGHT_PANEL_KINDS` becomes a type
        // error here rather than silently joining the set of surfaces that
        // close without releasing anything.
        surface satisfies never;
    }
  }

  return { dismissPlanSidebar, previewTabIds, terminalIds, forgottenSubagentChildIds };
}
