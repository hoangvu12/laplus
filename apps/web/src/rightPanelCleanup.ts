/**
 * What closing a right-panel surface has to undo outside the workspace.
 *
 * A right-panel tab is presentation, but two of the surface kinds are windows
 * onto something the server owns: a browser tab is a preview session and a
 * terminal tab is one or more shells. Closing those tabs must stop those
 * things; closing any other tab must not reach the server at all.
 *
 * The distinction used to live inside a `useCallback` in `ChatView`, where the
 * only way to find out whether closing a *child* tab could interrupt the child
 * was to read a loop and check that no branch matched. Naming the answer makes
 * it a value a test can hold: **the complete set of calls a close may make is
 * the three fields below**, so a surface kind that contributes to none of them
 * demonstrably asks the server for nothing.
 */
import type { RightPanelSurface } from "./rightPanelStore";

export interface RightPanelSurfaceCleanup {
  /** The plan sidebar is dismissed for the current turn with its surface. */
  readonly dismissPlanSidebar: boolean;
  /** Preview sessions to close, in the order their tabs appeared. */
  readonly previewTabIds: readonly string[];
  /** Terminal sessions to close, including every pane of a split surface. */
  readonly terminalIds: readonly string[];
}

export function resolveRightPanelSurfaceCleanup(
  surfaces: readonly RightPanelSurface[],
): RightPanelSurfaceCleanup {
  const previewTabIds: string[] = [];
  const terminalIds: string[] = [];
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
      // A child's work stream is recorded by the server whether or not anyone
      // is watching it, so hiding the view releases nothing and stops nothing.
      // Files, the explorer and the diff are equally free.
      case "subagent":
      case "file":
      case "files":
      case "diff":
        break;
    }
  }

  return { dismissPlanSidebar, previewTabIds, terminalIds };
}
