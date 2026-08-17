// @vitest-environment happy-dom
/**
 * Nothing a child does opens the right panel. Only a click does.
 *
 * "Background delegation never steals focus" is the criterion, and a criterion
 * about what does *not* happen has to be driven rather than reasoned about:
 * this mounts a compact child row through the same composition `ChatView`
 * performs and takes it through the whole of a child's life — appearing,
 * working, blocked, and every terminal state it can reach — with the real
 * store watching. The panel stays shut until the developer opens it.
 *
 * The guard this is worth keeping for is a future effect: "scroll the new child
 * into view", "focus the failing child", "surface the blocked one". Each is a
 * reasonable-sounding change, each would steal the workspace, and each turns
 * this red.
 */
import { scopeThreadRef } from "@t3tools/client-runtime/environment";
import { type EnvironmentId, ThreadId } from "@t3tools/contracts";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vite-plus/test";

import type { WorkLogEntry, WorkLogToolLifecycleStatus } from "../../session-logic";
import { openSubagentSurface, useRightPanelStore } from "../../rightPanelStore";
import { SimpleWorkEntryRow } from "./MessagesTimeline";

const threadRef = scopeThreadRef("env-1" as EnvironmentId, ThreadId.make("thread-A"));

function childRow(overrides: Partial<WorkLogEntry> = {}): WorkLogEntry {
  return {
    id: "work-call_task_1",
    createdAt: "2026-08-17T19:12:28.000Z",
    label: "Subagent explore",
    toolTitle: "Subagent explore",
    tone: "tool",
    itemType: "collab_agent_tool_call",
    subagentChildId: "call_task_1",
    ...overrides,
  };
}

/** Exactly what `ChatView` composes for an inline child row. */
const row = (entry: WorkLogEntry) => (
  <SimpleWorkEntryRow
    workEntry={entry}
    workspaceRoot={undefined}
    onOpenSubagent={(childId) => openSubagentSurface(threadRef, childId)}
  />
);

const workspace = () => useRightPanelStore.getState().byThreadKey;

beforeEach(() => {
  useRightPanelStore.setState({ byThreadKey: {} });
});

afterEach(() => {
  cleanup();
});

describe("a child running in the background", () => {
  it("opens no right-panel surface as it starts, works, blocks, and finishes", () => {
    const lifecycle: ReadonlyArray<Partial<WorkLogEntry>> = [
      // Pending: recorded, with nothing to say yet.
      {},
      { toolLifecycleStatus: "inProgress", detail: "Reading src/index.ts" },
      // Blocked on a permission it cannot answer itself.
      { toolLifecycleStatus: "inProgress", requestKind: "command", detail: "Waiting on approval" },
      { toolLifecycleStatus: "completed", detail: "eleven files" },
    ];

    const view = render(row(childRow(lifecycle[0])));
    for (const update of lifecycle.slice(1)) {
      view.rerender(row(childRow(update)));
      expect(workspace()).toEqual({});
    }

    expect(workspace()).toEqual({});
  });

  it("opens nothing when it fails, is declined, or is stopped", () => {
    const terminal: ReadonlyArray<WorkLogToolLifecycleStatus> = ["failed", "declined", "stopped"];

    const view = render(row(childRow({ toolLifecycleStatus: "inProgress" })));
    for (const status of terminal) {
      view.rerender(row(childRow({ toolLifecycleStatus: status, tone: "error" })));
      expect(workspace()).toEqual({});
    }
  });

  it("opens nothing when several children start at once", () => {
    const first = render(row(childRow({ toolLifecycleStatus: "inProgress" })));
    render(
      row(
        childRow({
          id: "work-call_task_2",
          subagentChildId: "call_task_2",
          label: "Subagent general",
          toolTitle: "Subagent general",
          toolLifecycleStatus: "inProgress",
        }),
      ),
    );

    expect(workspace()).toEqual({});
    first.unmount();
    expect(workspace()).toEqual({});
  });

  /**
   * And the other half of the claim: the panel is not merely never open, it is
   * one click away. Without this the tests above would pass on a row that was
   * wired to nothing at all.
   */
  it("opens its tab, and only its tab, when the developer clicks the row", () => {
    render(row(childRow({ toolLifecycleStatus: "completed" })));

    fireEvent.click(screen.getByRole("button", { name: /Open subagent work stream/ }));

    expect(workspace()).toEqual({
      "env-1:thread-A": {
        isOpen: true,
        activeSurfaceId: "subagent:call_task_1",
        surfaces: [{ id: "subagent:call_task_1", kind: "subagent", resourceId: "call_task_1" }],
      },
    });
  });
});
