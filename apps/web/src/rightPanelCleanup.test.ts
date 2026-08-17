import { describe, expect, it } from "vite-plus/test";

import { cleanupReachesTheServer, resolveRightPanelSurfaceCleanup } from "./rightPanelCleanup";
import type { RightPanelSurface } from "./rightPanelStore";

const NOTHING = {
  dismissPlanSidebar: false,
  previewTabIds: [],
  terminalIds: [],
  forgottenSubagentChildIds: [],
};

const subagent = (childId: string): RightPanelSurface => ({
  id: `subagent:${childId}`,
  kind: "subagent",
  resourceId: childId,
});

const terminal = (...terminalIds: [string, ...string[]]): RightPanelSurface => ({
  id: `terminal:${terminalIds[0]}`,
  kind: "terminal",
  resourceId: terminalIds[0],
  terminalIds,
  activeTerminalId: terminalIds[0],
});

describe("what closing a right-panel surface entails", () => {
  it("stops the preview session behind a browser tab", () => {
    expect(
      resolveRightPanelSurfaceCleanup([
        { id: "browser:tab-a", kind: "preview", resourceId: "tab-a" },
        { id: "browser:new", kind: "preview", resourceId: null },
      ]),
    ).toEqual({ ...NOTHING, previewTabIds: ["tab-a"] });
  });

  it("stops every pane of a terminal surface", () => {
    expect(resolveRightPanelSurfaceCleanup([terminal("term-1", "term-2")])).toEqual({
      ...NOTHING,
      terminalIds: ["term-1", "term-2"],
    });
  });

  it("dismisses the plan sidebar with the plan surface", () => {
    expect(resolveRightPanelSurfaceCleanup([{ id: "plan", kind: "plan" }])).toEqual({
      ...NOTHING,
      dismissPlanSidebar: true,
    });
  });

  /**
   * The criterion, stated as the shape of this value rather than as an argument
   * about the code: closing a subagent tab releases the reader's place in the
   * stream and does nothing else. There is no interrupt, no cancellation, no
   * detachment and no provider call to make, because the complete set of calls
   * that leave this window is the three fields `cleanupReachesTheServer` reads
   * and a subagent contributes to none of them.
   */
  it("releases only the reader's place when a subagent tab is closed", () => {
    const one = resolveRightPanelSurfaceCleanup([subagent("call_task_1")]);
    expect(one).toEqual({ ...NOTHING, forgottenSubagentChildIds: ["call_task_1"] });
    expect(cleanupReachesTheServer(one)).toBe(false);

    const several = resolveRightPanelSurfaceCleanup([
      subagent("call_task_1"),
      subagent("call_task_2"),
    ]);
    expect(several.forgottenSubagentChildIds).toEqual(["call_task_1", "call_task_2"]);
    expect(cleanupReachesTheServer(several)).toBe(false);
  });

  it("closes a subagent beside a terminal without asking anything of the child", () => {
    const cleanup = resolveRightPanelSurfaceCleanup([subagent("call_task_1"), terminal("term-1")]);

    expect(cleanup).toEqual({
      ...NOTHING,
      terminalIds: ["term-1"],
      forgottenSubagentChildIds: ["call_task_1"],
    });
    // The terminal is what reaches the server here; the child contributed
    // nothing to that.
    expect(cleanupReachesTheServer(cleanup)).toBe(true);
    expect(
      cleanupReachesTheServer(resolveRightPanelSurfaceCleanup([subagent("call_task_1")])),
    ).toBe(false);
  });

  it("asks for nothing when a file, an explorer or a diff tab is closed", () => {
    expect(
      resolveRightPanelSurfaceCleanup([
        {
          id: "file:src/index.ts",
          kind: "file",
          relativePath: "src/index.ts",
          revealLine: null,
          revealRequestId: 1,
        },
        { id: "files", kind: "files" },
        { id: "diff", kind: "diff" },
      ]),
    ).toEqual(NOTHING);
  });

  it("asks for nothing when nothing is closed", () => {
    const cleanup = resolveRightPanelSurfaceCleanup([]);
    expect(cleanup).toEqual(NOTHING);
    expect(cleanupReachesTheServer(cleanup)).toBe(false);
  });
});
