import { describe, expect, it } from "vite-plus/test";

import { resolveRightPanelSurfaceCleanup } from "./rightPanelCleanup";
import type { RightPanelSurface } from "./rightPanelStore";

const NOTHING = { dismissPlanSidebar: false, previewTabIds: [], terminalIds: [] };

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

describe("what closing a right-panel surface asks the server for", () => {
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
   * The criterion, stated as the only thing this seam can say: closing a child
   * tab asks for nothing. There is no interrupt, no cancellation, no
   * detachment and no provider call to make, because the complete set of calls
   * a close can make is the three fields of this value and a child contributes
   * to none of them.
   */
  it("asks for nothing at all when a child tab is closed", () => {
    expect(resolveRightPanelSurfaceCleanup([subagent("call_task_1")])).toEqual(NOTHING);
    expect(
      resolveRightPanelSurfaceCleanup([subagent("call_task_1"), subagent("call_task_2")]),
    ).toEqual(NOTHING);
  });

  it("closes a child beside a terminal without touching the child", () => {
    expect(resolveRightPanelSurfaceCleanup([subagent("call_task_1"), terminal("term-1")])).toEqual({
      ...NOTHING,
      terminalIds: ["term-1"],
    });
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
    expect(resolveRightPanelSurfaceCleanup([])).toEqual(NOTHING);
  });
});
