import { scopeThreadRef } from "@t3tools/client-runtime/environment";
import { type EnvironmentId, ThreadId } from "@t3tools/contracts";
import { afterEach, beforeEach, describe, expect, it } from "vite-plus/test";
import { createJSONStorage } from "zustand/middleware";

import { createMemoryStorage } from "./lib/storage";
import {
  migratePersistedRightPanelState,
  selectActiveRightPanel,
  selectActiveRightPanelSurface,
  selectThreadRightPanelState,
  useRightPanelStore,
} from "./rightPanelStore";

const refA = scopeThreadRef("env-1" as EnvironmentId, ThreadId.make("thread-A"));
const refB = scopeThreadRef("env-1" as EnvironmentId, ThreadId.make("thread-B"));

beforeEach(() => {
  useRightPanelStore.setState({ byThreadKey: {} });
});

describe("rightPanelStore", () => {
  it("drops the legacy singleton terminal surface during migration", () => {
    expect(
      migratePersistedRightPanelState({
        byThreadKey: {
          "env-1:thread-A": {
            activeSurfaceId: "terminal",
            surfaces: [
              { id: "browser:tab-a", kind: "preview", resourceId: "tab-a" },
              { id: "terminal", kind: "terminal" },
            ],
          },
        },
      }),
    ).toEqual({
      byThreadKey: {
        "env-1:thread-A": {
          isOpen: false,
          activeSurfaceId: null,
          surfaces: [{ id: "browser:tab-a", kind: "preview", resourceId: "tab-a" }],
        },
      },
    });
  });

  it("upgrades saved single-session terminal surfaces to split-capable surfaces", () => {
    expect(
      migratePersistedRightPanelState({
        byThreadKey: {
          "env-1:thread-A": {
            isOpen: true,
            activeSurfaceId: "terminal:term-1",
            surfaces: [{ id: "terminal:term-1", kind: "terminal", resourceId: "term-1" }],
          },
        },
      }),
    ).toEqual({
      byThreadKey: {
        "env-1:thread-A": {
          isOpen: true,
          activeSurfaceId: "terminal:term-1",
          surfaces: [
            {
              id: "terminal:term-1",
              kind: "terminal",
              resourceId: "term-1",
              terminalIds: ["term-1"],
              activeTerminalId: "term-1",
            },
          ],
        },
      },
    });
  });

  it("upgrades saved file surfaces with neutral reveal state", () => {
    expect(
      migratePersistedRightPanelState({
        byThreadKey: {
          "env-1:thread-A": {
            isOpen: true,
            activeSurfaceId: "file:src/index.ts",
            surfaces: [{ id: "file:src/index.ts", kind: "file", relativePath: "src/index.ts" }],
          },
        },
      }),
    ).toEqual({
      byThreadKey: {
        "env-1:thread-A": {
          isOpen: true,
          activeSurfaceId: "file:src/index.ts",
          surfaces: [
            {
              id: "file:src/index.ts",
              kind: "file",
              relativePath: "src/index.ts",
              revealLine: null,
              revealRequestId: 0,
            },
          ],
        },
      },
    });
  });

  it("open sets the active panel for a thread", () => {
    useRightPanelStore.getState().open(refA, "preview");
    expect(selectActiveRightPanel(useRightPanelStore.getState().byThreadKey, refA)).toBe("preview");
    expect(selectActiveRightPanel(useRightPanelStore.getState().byThreadKey, refB)).toBeNull();
  });

  it("opening a different kind keeps both surfaces and activates the new one", () => {
    useRightPanelStore.getState().open(refA, "plan");
    useRightPanelStore.getState().open(refA, "preview");
    expect(selectActiveRightPanel(useRightPanelStore.getState().byThreadKey, refA)).toBe("preview");
    expect(
      selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA).surfaces,
    ).toHaveLength(2);
  });

  it("reopening an inactive singleton activates its existing surface", () => {
    useRightPanelStore.getState().open(refA, "diff");
    useRightPanelStore.getState().open(refA, "plan");
    useRightPanelStore.getState().open(refA, "diff");

    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: true,
      activeSurfaceId: "diff",
      surfaces: [
        { id: "diff", kind: "diff" },
        { id: "plan", kind: "plan" },
      ],
    });
  });

  it("keeps files as a singleton surface", () => {
    useRightPanelStore.getState().open(refA, "files");
    useRightPanelStore.getState().open(refA, "files");
    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: true,
      activeSurfaceId: "files",
      surfaces: [{ id: "files", kind: "files" }],
    });
  });

  it("replaces the standalone explorer with peer file surfaces", () => {
    useRightPanelStore.getState().open(refA, "files");
    useRightPanelStore.getState().openFile(refA, "src/index.ts");
    useRightPanelStore.getState().openFile(refA, "src/index.ts");
    useRightPanelStore.getState().openFile(refA, "README.md");

    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: true,
      activeSurfaceId: "file:README.md",
      surfaces: [
        {
          id: "file:src/index.ts",
          kind: "file",
          relativePath: "src/index.ts",
          revealLine: null,
          revealRequestId: 2,
        },
        {
          id: "file:README.md",
          kind: "file",
          relativePath: "README.md",
          revealLine: null,
          revealRequestId: 1,
        },
      ],
    });
  });

  it("updates line reveal requests when reopening a file surface", () => {
    useRightPanelStore.getState().openFile(refA, "src/index.ts", 42);
    useRightPanelStore.getState().openFile(refA, "src/index.ts", 87);

    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: true,
      activeSurfaceId: "file:src/index.ts",
      surfaces: [
        {
          id: "file:src/index.ts",
          kind: "file",
          relativePath: "src/index.ts",
          revealLine: 87,
          revealRequestId: 2,
        },
      ],
    });

    useRightPanelStore.getState().openFile(refA, "src/index.ts");

    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: true,
      activeSurfaceId: "file:src/index.ts",
      surfaces: [
        {
          id: "file:src/index.ts",
          kind: "file",
          relativePath: "src/index.ts",
          revealLine: null,
          revealRequestId: 3,
        },
      ],
    });
  });

  it("removes persisted file surfaces when their workspace no longer exists", () => {
    useRightPanelStore.getState().openFile(refA, "src/index.ts");
    useRightPanelStore.getState().open(refA, "plan");
    useRightPanelStore.getState().openFile(refA, "README.md");

    useRightPanelStore.getState().reconcileFileSurfaces(refA, false);

    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: true,
      activeSurfaceId: "plan",
      surfaces: [{ id: "plan", kind: "plan" }],
    });

    useRightPanelStore.getState().openFile(refB, "conductor.json");
    useRightPanelStore.getState().reconcileFileSurfaces(refB, false);
    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refB)).toEqual({
      isOpen: false,
      activeSurfaceId: null,
      surfaces: [],
    });
  });

  it("close hides the panel without clearing its selected surface", () => {
    useRightPanelStore.getState().open(refA, "plan");
    useRightPanelStore.getState().close(refA);
    expect(selectActiveRightPanel(useRightPanelStore.getState().byThreadKey, refA)).toBeNull();
    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: false,
      activeSurfaceId: "plan",
      surfaces: [{ id: "plan", kind: "plan" }],
    });
  });

  it("toggles empty panel visibility without creating a surface", () => {
    useRightPanelStore.getState().toggleVisibility(refA);
    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: true,
      activeSurfaceId: null,
      surfaces: [],
    });

    useRightPanelStore.getState().toggleVisibility(refA);
    expect(useRightPanelStore.getState().byThreadKey).toEqual({});
  });

  it("toggle hides the panel without discarding the active surface", () => {
    useRightPanelStore.getState().toggle(refA, "diff");
    expect(selectActiveRightPanel(useRightPanelStore.getState().byThreadKey, refA)).toBe("diff");
    useRightPanelStore.getState().toggle(refA, "diff");
    expect(selectActiveRightPanel(useRightPanelStore.getState().byThreadKey, refA)).toBeNull();
    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: false,
      activeSurfaceId: "diff",
      surfaces: [{ id: "diff", kind: "diff" }],
    });
  });

  it("toggle to a different kind switches active", () => {
    useRightPanelStore.getState().toggle(refA, "preview");
    useRightPanelStore.getState().toggle(refA, "plan");
    expect(selectActiveRightPanel(useRightPanelStore.getState().byThreadKey, refA)).toBe("plan");
  });

  it("removeThread clears persisted state", () => {
    useRightPanelStore.getState().open(refA, "plan");
    useRightPanelStore.getState().removeThread(refA);
    expect(selectActiveRightPanel(useRightPanelStore.getState().byThreadKey, refA)).toBeNull();
  });

  it("close on never-opened thread is a no-op", () => {
    useRightPanelStore.getState().close(refA);
    expect(useRightPanelStore.getState().byThreadKey).toEqual({});
  });

  it("tracks one surface per browser session", () => {
    useRightPanelStore.getState().openBrowser(refA, "tab-a");
    useRightPanelStore.getState().openBrowser(refA, "tab-b");

    const state = selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA);
    expect(state.surfaces.map((surface) => surface.id)).toEqual(["browser:tab-a", "browser:tab-b"]);
    expect(selectActiveRightPanelSurface(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      id: "browser:tab-b",
      kind: "preview",
      resourceId: "tab-b",
    });
  });

  it("tracks one surface per terminal session", () => {
    useRightPanelStore.getState().openTerminal(refA, "term-1");
    useRightPanelStore.getState().openTerminal(refA, "term-2");

    const state = selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA);
    expect(state.surfaces).toEqual([
      {
        id: "terminal:term-1",
        kind: "terminal",
        resourceId: "term-1",
        terminalIds: ["term-1"],
        activeTerminalId: "term-1",
      },
      {
        id: "terminal:term-2",
        kind: "terminal",
        resourceId: "term-2",
        terminalIds: ["term-2"],
        activeTerminalId: "term-2",
      },
    ]);
    expect(state.activeSurfaceId).toBe("terminal:term-2");
  });

  it("tracks split panes and the active pane within a terminal surface", () => {
    useRightPanelStore.getState().openTerminal(refA, "term-1");
    useRightPanelStore.getState().splitTerminal(refA, "terminal:term-1", "term-2");

    expect(selectActiveRightPanelSurface(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      id: "terminal:term-1",
      kind: "terminal",
      resourceId: "term-1",
      terminalIds: ["term-1", "term-2"],
      activeTerminalId: "term-2",
    });

    useRightPanelStore.getState().activateTerminal(refA, "terminal:term-1", "term-1");
    useRightPanelStore.getState().closeTerminal(refA, "terminal:term-1", "term-1");
    expect(selectActiveRightPanelSurface(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      id: "terminal:term-1",
      kind: "terminal",
      resourceId: "term-1",
      terminalIds: ["term-2"],
      activeTerminalId: "term-2",
    });
  });

  it("tracks vertical layout for a terminal surface", () => {
    useRightPanelStore.getState().openTerminal(refA, "term-1");
    useRightPanelStore.getState().splitTerminal(refA, "terminal:term-1", "term-2", "vertical");

    expect(selectActiveRightPanelSurface(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      id: "terminal:term-1",
      kind: "terminal",
      resourceId: "term-1",
      terminalIds: ["term-1", "term-2"],
      activeTerminalId: "term-2",
      splitDirection: "vertical",
    });
  });

  it("closing the final terminal pane removes its surface and closes the panel", () => {
    useRightPanelStore.getState().openTerminal(refA, "term-1");
    useRightPanelStore.getState().closeTerminal(refA, "terminal:term-1", "term-1");

    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: false,
      activeSurfaceId: null,
      surfaces: [],
    });
  });

  it("closing the active surface activates a neighboring surface", () => {
    useRightPanelStore.getState().openBrowser(refA, "tab-a");
    useRightPanelStore.getState().openTerminal(refA, "term-1");
    useRightPanelStore.getState().closeSurface(refA, "terminal:term-1");

    expect(selectActiveRightPanelSurface(useRightPanelStore.getState().byThreadKey, refA)?.id).toBe(
      "browser:tab-a",
    );
  });

  it("closing the final surface closes the panel", () => {
    useRightPanelStore.getState().openTerminal(refA, "term-1");
    useRightPanelStore.getState().closeSurface(refA, "terminal:term-1");

    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: false,
      activeSurfaceId: null,
      surfaces: [],
    });
  });

  it("closing other surfaces keeps the selected surface active", () => {
    useRightPanelStore.getState().openBrowser(refA, "tab-a");
    useRightPanelStore.getState().openFile(refA, "src/index.ts");
    useRightPanelStore.getState().openTerminal(refA, "term-1");

    useRightPanelStore.getState().closeOtherSurfaces(refA, "file:src/index.ts");

    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: true,
      activeSurfaceId: "file:src/index.ts",
      surfaces: [
        {
          id: "file:src/index.ts",
          kind: "file",
          relativePath: "src/index.ts",
          revealLine: null,
          revealRequestId: 1,
        },
      ],
    });
  });

  it("closing surfaces to the right activates the selected surface when active was removed", () => {
    useRightPanelStore.getState().openBrowser(refA, "tab-a");
    useRightPanelStore.getState().openFile(refA, "src/index.ts");
    useRightPanelStore.getState().openTerminal(refA, "term-1");

    useRightPanelStore.getState().closeSurfacesToRight(refA, "browser:tab-a");

    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: true,
      activeSurfaceId: "browser:tab-a",
      surfaces: [{ id: "browser:tab-a", kind: "preview", resourceId: "tab-a" }],
    });
  });

  it("closing all surfaces closes the panel", () => {
    useRightPanelStore.getState().openBrowser(refA, "tab-a");
    useRightPanelStore.getState().openFile(refA, "src/index.ts");

    useRightPanelStore.getState().closeAllSurfaces(refA);

    expect(selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA)).toEqual({
      isOpen: false,
      activeSurfaceId: null,
      surfaces: [],
    });
  });

  /**
   * A subagent surface is resource-addressed like a file or a terminal, so
   * opening the same child twice activates the tab it already has and opening
   * another child adds one — the workspace's existing rules, unchanged.
   */
  it("opens a child work stream as a tab and activates it rather than duplicating", () => {
    useRightPanelStore.getState().openSubagent(refA, "call_task_1");
    useRightPanelStore.getState().openSubagent(refA, "call_task_2");
    useRightPanelStore.getState().openSubagent(refA, "call_task_1");

    const state = selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA);
    expect(state.surfaces.map((surface) => surface.id)).toEqual([
      "subagent:call_task_1",
      "subagent:call_task_2",
    ]);
    expect(state.activeSurfaceId).toBe("subagent:call_task_1");
    expect(state.isOpen).toBe(true);
  });

  it("keeps child tabs beside every other surface kind", () => {
    useRightPanelStore.getState().openTerminal(refA, "term-1");
    useRightPanelStore.getState().openSubagent(refA, "call_task_1");
    useRightPanelStore.getState().openFile(refA, "src/index.ts");
    useRightPanelStore.getState().open(refA, "diff");
    useRightPanelStore.getState().openBrowser(refA, "tab-a");

    expect(
      selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA).surfaces.map(
        (surface) => surface.kind,
      ),
    ).toEqual(["terminal", "subagent", "file", "diff", "preview"]);
  });

  /**
   * Closing is presentation only. This store cannot send a provider command, so
   * what the test can state is the whole of what closing does here: the surface
   * goes and the neighbour it left becomes active.
   */
  it("closing a child tab hides only that surface", () => {
    useRightPanelStore.getState().openSubagent(refA, "call_task_1");
    useRightPanelStore.getState().openSubagent(refA, "call_task_2");
    useRightPanelStore.getState().closeSurface(refA, "subagent:call_task_2");

    const closed = selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA);
    expect(closed.surfaces.map((surface) => surface.id)).toEqual(["subagent:call_task_1"]);
    expect(closed.activeSurfaceId).toBe("subagent:call_task_1");

    // And the inline row reopens the same stream, at the same address.
    useRightPanelStore.getState().openSubagent(refA, "call_task_2");
    expect(
      selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA).activeSurfaceId,
    ).toBe("subagent:call_task_2");
  });

  it("restores persisted child tabs and drops one nothing can address", () => {
    expect(
      migratePersistedRightPanelState({
        byThreadKey: {
          "env-1:thread-A": {
            isOpen: true,
            activeSurfaceId: "subagent:call_task_1",
            surfaces: [
              { id: "subagent:call_task_1", kind: "subagent", resourceId: "call_task_1" },
              { id: "subagent:mismatched", kind: "subagent", resourceId: "call_task_9" },
            ],
          },
        },
      }),
    ).toEqual({
      byThreadKey: {
        "env-1:thread-A": {
          isOpen: true,
          activeSurfaceId: "subagent:call_task_1",
          surfaces: [{ id: "subagent:call_task_1", kind: "subagent", resourceId: "call_task_1" }],
        },
      },
    });
  });

  /**
   * "Several children coexist with files, diffs, terminals, previews and
   * plans" — the whole list, in one workspace, with the children keeping their
   * places among it. `files` and `file` are the one pair that cannot both be
   * present: opening a file replaces the standalone explorer, which is the
   * workspace's existing rule and not something child tabs change.
   */
  it("keeps several child tabs among every other surface kind", () => {
    useRightPanelStore.getState().openTerminal(refA, "term-1");
    useRightPanelStore.getState().openSubagent(refA, "call_task_1");
    useRightPanelStore.getState().open(refA, "diff");
    useRightPanelStore.getState().openBrowser(refA, "tab-a");
    useRightPanelStore.getState().openSubagent(refA, "call_task_2");
    useRightPanelStore.getState().open(refA, "plan");
    useRightPanelStore.getState().openSubagent(refA, "call_task_3");
    useRightPanelStore.getState().open(refA, "files");

    expect(
      selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA).surfaces.map(
        (surface) => surface.id,
      ),
    ).toEqual([
      "terminal:term-1",
      "subagent:call_task_1",
      "diff",
      "browser:tab-a",
      "subagent:call_task_2",
      "plan",
      "subagent:call_task_3",
      "files",
    ]);

    // And a file tab takes the explorer's place without disturbing the children.
    useRightPanelStore.getState().openFile(refA, "src/index.ts");
    const state = selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA);
    expect(state.surfaces.map((surface) => surface.id)).toEqual([
      "terminal:term-1",
      "subagent:call_task_1",
      "diff",
      "browser:tab-a",
      "subagent:call_task_2",
      "plan",
      "subagent:call_task_3",
      "file:src/index.ts",
    ]);
    expect(state.activeSurfaceId).toBe("file:src/index.ts");
  });

  /**
   * Opening is append-and-activate; activating an open child moves nothing.
   * Both are the workspace's existing rules for a resource-addressed surface,
   * and the point of the test is that a child obeys them rather than sorting
   * itself anywhere special.
   */
  it("adds each new child at the end and never reorders one that is already open", () => {
    useRightPanelStore.getState().openSubagent(refA, "call_task_1");
    useRightPanelStore.getState().openFile(refA, "src/index.ts");
    useRightPanelStore.getState().openSubagent(refA, "call_task_2");

    const order = ["subagent:call_task_1", "file:src/index.ts", "subagent:call_task_2"];
    expect(
      selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA).surfaces.map(
        (surface) => surface.id,
      ),
    ).toEqual(order);

    useRightPanelStore.getState().openSubagent(refA, "call_task_1");
    const activated = selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA);
    expect(activated.surfaces.map((surface) => surface.id)).toEqual(order);
    expect(activated.activeSurfaceId).toBe("subagent:call_task_1");
  });

  /**
   * A child tab is not reconciled away by anything that prunes surfaces whose
   * resource has gone. That is what keeps an unresolvable restored child on
   * screen as an explicit unavailable surface — `SubagentStreamPanel.test.tsx`
   * proves what that surface then says — rather than a tab that vanishes and
   * leaves the developer wondering whether they imagined it.
   */
  it("never prunes a child tab whose stream it cannot vouch for", () => {
    useRightPanelStore.getState().openSubagent(refA, "call_task_1");
    useRightPanelStore.getState().openFile(refA, "src/index.ts");
    useRightPanelStore.getState().openBrowser(refA, "tab-a");
    useRightPanelStore.getState().openSubagent(refA, "call_task_2");

    useRightPanelStore.getState().reconcileFileSurfaces(refA, false);
    useRightPanelStore.getState().reconcileBrowserSurfaces(refA, []);

    expect(
      selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA).surfaces,
    ).toEqual([
      { id: "subagent:call_task_1", kind: "subagent", resourceId: "call_task_1" },
      { id: "subagent:call_task_2", kind: "subagent", resourceId: "call_task_2" },
    ]);
  });

  it("reconciles browser surfaces without deleting other surface kinds", () => {
    useRightPanelStore.getState().openTerminal(refA, "term-1");
    useRightPanelStore.getState().openBrowser(refA, "tab-a");
    useRightPanelStore.getState().openBrowser(refA, "tab-b");
    useRightPanelStore.getState().reconcileBrowserSurfaces(refA, ["tab-b", "tab-c"]);

    expect(
      selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA).surfaces.map(
        (surface) => surface.id,
      ),
    ).toEqual(["terminal:term-1", "browser:tab-b", "browser:tab-c"]);
  });
});

/**
 * Restoration, through the persistence this store actually configures rather
 * than through its migration helper alone. What a reload does is: read back
 * what the last write left, at the version it was written under. Anything the
 * store forgets between those two moments is a tab the developer lost.
 */
describe("a right-panel workspace across a reload", () => {
  /** Stands in for `localStorage`, and can be reloaded from a captured write. */
  function browserStorage() {
    let written: string | null = null;
    return {
      getItem: () => written,
      setItem: (_name: string, value: string) => {
        written = value;
      },
      removeItem: () => {
        written = null;
      },
      /** What a reload would find. */
      captured: () => written,
      /** Put it back, after the page has gone away. */
      restore: (value: string) => {
        written = value;
      },
    };
  }

  afterEach(() => {
    useRightPanelStore.persist.setOptions({
      storage: createJSONStorage(() => createMemoryStorage()),
    });
  });

  it("restores child tabs, their order and the active tab, carrying no stream with them", async () => {
    const storage = browserStorage();
    useRightPanelStore.persist.setOptions({ storage: createJSONStorage(() => storage) });

    useRightPanelStore.getState().openSubagent(refA, "call_task_1");
    useRightPanelStore.getState().openFile(refA, "src/index.ts");
    useRightPanelStore.getState().openSubagent(refA, "call_task_2");
    useRightPanelStore.getState().openTerminal(refA, "term-1");
    useRightPanelStore.getState().openSubagent(refA, "call_task_1");
    const captured = storage.captured();
    expect(captured).not.toBeNull();

    // The window closes and reopens: the store is new and empty, and the only
    // thing that survived is what was written.
    useRightPanelStore.setState({ byThreadKey: {} });
    storage.restore(captured!);
    await useRightPanelStore.persist.rehydrate();

    const restored = selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA);
    expect(restored.surfaces.map((surface) => surface.id)).toEqual([
      "subagent:call_task_1",
      "file:src/index.ts",
      "subagent:call_task_2",
      "terminal:term-1",
    ]);
    expect(restored.activeSurfaceId).toBe("subagent:call_task_1");
    expect(restored.isOpen).toBe(true);

    // Lazily loaded, stated as what was persisted: a restored child tab is a
    // reference and nothing more, so opening the workspace fetches no stream
    // until a surface is mounted.
    expect(restored.surfaces[0]).toEqual({
      id: "subagent:call_task_1",
      kind: "subagent",
      resourceId: "call_task_1",
    });
    expect(restored.surfaces[2]).toEqual({
      id: "subagent:call_task_2",
      kind: "subagent",
      resourceId: "call_task_2",
    });
  });

  /** A workspace is per thread, and so is what comes back with it. */
  it("restores each thread's child tabs to that thread", async () => {
    const storage = browserStorage();
    useRightPanelStore.persist.setOptions({ storage: createJSONStorage(() => storage) });

    useRightPanelStore.getState().openSubagent(refA, "call_task_1");
    useRightPanelStore.getState().openSubagent(refB, "call_task_2");
    const captured = storage.captured();

    useRightPanelStore.setState({ byThreadKey: {} });
    storage.restore(captured!);
    await useRightPanelStore.persist.rehydrate();

    expect(
      selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA).surfaces.map(
        (surface) => surface.id,
      ),
    ).toEqual(["subagent:call_task_1"]);
    expect(
      selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refB).surfaces.map(
        (surface) => surface.id,
      ),
    ).toEqual(["subagent:call_task_2"]);
  });

  /**
   * A closed tab stays closed. Reopening is the inline row's job, which is what
   * keeps "closing hides the view" from quietly meaning "until you reload".
   */
  it("does not bring back a child tab that was closed before the reload", async () => {
    const storage = browserStorage();
    useRightPanelStore.persist.setOptions({ storage: createJSONStorage(() => storage) });

    useRightPanelStore.getState().openSubagent(refA, "call_task_1");
    useRightPanelStore.getState().openSubagent(refA, "call_task_2");
    useRightPanelStore.getState().closeSurface(refA, "subagent:call_task_1");
    const captured = storage.captured();

    useRightPanelStore.setState({ byThreadKey: {} });
    storage.restore(captured!);
    await useRightPanelStore.persist.rehydrate();

    expect(
      selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, refA).surfaces.map(
        (surface) => surface.id,
      ),
    ).toEqual(["subagent:call_task_2"]);
  });
});
