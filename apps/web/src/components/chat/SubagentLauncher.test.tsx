// @vitest-environment happy-dom
/**
 * The link between an inline child row and the workspace tab it opens.
 *
 * Every other test of this feature stops at one end of that link or the other:
 * `MessagesTimeline.test.tsx` proves the row carries the affordance,
 * `MessagesTimeline.logic.test.ts` proves it decides to launch rather than
 * expand, and `rightPanelStore.test.ts` proves the store opens or activates.
 * This drives the whole of it — a real click on the real row, through the same
 * composition `ChatView` performs (`openSubagentSurface(threadRef, childId)`),
 * against the real store.
 *
 * Ticket 01's third and fifth criteria are that link. What is deliberately
 * still not exercised here is the browser: ticket 07 owns the cross-provider
 * acceptance run against a running Laplus.
 */
import { scopeThreadRef } from "@t3tools/client-runtime/environment";
import { type EnvironmentId, ThreadId } from "@t3tools/contracts";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vite-plus/test";

import {
  openSubagentSurface,
  selectThreadRightPanelState,
  useRightPanelStore,
} from "../../rightPanelStore";
import { SimpleWorkEntryRow } from "./MessagesTimeline";

const threadRef = scopeThreadRef("env-1" as EnvironmentId, ThreadId.make("thread-A"));

function childRow(childId: string | undefined, title: string) {
  return {
    id: `work-${childId ?? "none"}`,
    createdAt: "2026-08-17T19:12:28.000Z",
    label: title,
    toolTitle: title,
    tone: "tool" as const,
    itemType: "collab_agent_tool_call" as const,
    ...(childId === undefined ? {} : { subagentChildId: childId }),
  };
}

/** Exactly what `ChatView` composes: the active thread, plus the row's child. */
function renderRow(childId: string | undefined, title = "Subagent explore") {
  return render(
    <SimpleWorkEntryRow
      workEntry={childRow(childId, title)}
      workspaceRoot={undefined}
      onOpenSubagent={(clicked) => openSubagentSurface(threadRef, clicked)}
    />,
  );
}

const surfaces = () =>
  selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, threadRef);

beforeEach(() => {
  useRightPanelStore.setState({ byThreadKey: {} });
});

afterEach(() => {
  cleanup();
});

describe("activating an inline child row", () => {
  it("opens that child's work stream as a right-panel tab", () => {
    renderRow("call_task_1");

    fireEvent.click(screen.getByRole("button", { name: /Open subagent work stream/ }));

    const state = surfaces();
    expect(state.surfaces).toEqual([
      { id: "subagent:call_task_1", kind: "subagent", resourceId: "call_task_1" },
    ]);
    expect(state.activeSurfaceId).toBe("subagent:call_task_1");
    expect(state.isOpen).toBe(true);
  });

  /** The criterion's own words: activates the same tab rather than duplicating. */
  it("activates the tab it already has rather than duplicating it", () => {
    const first = renderRow("call_task_1");
    fireEvent.click(screen.getByRole("button", { name: /Open subagent work stream/ }));
    first.unmount();

    // A sibling worker opens beside it, so the first child's tab is no longer
    // the active one when its row is clicked again.
    const second = renderRow("call_task_2", "Subagent general");
    fireEvent.click(screen.getByRole("button", { name: /Open subagent work stream/ }));
    second.unmount();
    expect(surfaces().activeSurfaceId).toBe("subagent:call_task_2");

    renderRow("call_task_1");
    fireEvent.click(screen.getByRole("button", { name: /Open subagent work stream/ }));

    const state = surfaces();
    expect(state.surfaces.map((surface) => surface.id)).toEqual([
      "subagent:call_task_1",
      "subagent:call_task_2",
    ]);
    expect(state.activeSurfaceId).toBe("subagent:call_task_1");
  });

  /**
   * The fifth criterion: closing the tab hid the view and nothing else, so the
   * row still reopens the same child's stream.
   */
  it("reopens the same child's stream after its tab has been closed", () => {
    renderRow("call_task_1");
    fireEvent.click(screen.getByRole("button", { name: /Open subagent work stream/ }));

    useRightPanelStore.getState().closeSurface(threadRef, "subagent:call_task_1");
    expect(surfaces().surfaces).toEqual([]);

    fireEvent.click(screen.getByRole("button", { name: /Open subagent work stream/ }));

    const state = surfaces();
    expect(state.surfaces).toEqual([
      { id: "subagent:call_task_1", kind: "subagent", resourceId: "call_task_1" },
    ]);
    expect(state.activeSurfaceId).toBe("subagent:call_task_1");
  });

  /** Keyboard and pointer reach the same behaviour, from the same row. */
  it("opens from the keyboard as well as the pointer", () => {
    renderRow("call_task_1");

    fireEvent.keyDown(screen.getByRole("button", { name: /Open subagent work stream/ }), {
      key: "Enter",
    });

    expect(surfaces().activeSurfaceId).toBe("subagent:call_task_1");
  });

  /** A driver that records no stream has nothing to open, and offers nothing. */
  it("opens nothing from a row with no work stream", () => {
    renderRow(undefined);

    expect(screen.queryByRole("button", { name: /Open subagent work stream/ })).toBeNull();
    expect(surfaces().surfaces).toEqual([]);
  });

  /** No conversation, no workspace to open into. */
  it("opens nothing when no conversation is active", () => {
    openSubagentSurface(null, "call_task_1");

    expect(useRightPanelStore.getState().byThreadKey).toEqual({});
  });
});
