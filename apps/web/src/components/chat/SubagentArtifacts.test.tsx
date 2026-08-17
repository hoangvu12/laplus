// @vitest-environment happy-dom
/**
 * The link between a file a child touched and the workspace tab it opens.
 *
 * The two ends are proved apart elsewhere — `SubagentStreamPanel.test.tsx` that
 * the affordance is drawn from the paths an entry carries, and
 * `subagentFileActions.test.ts` that the store opens a neighbour rather than a
 * replacement. This drives the whole of it: a real click on the real surface,
 * against the real store, with the child's own tab already open beside it.
 *
 * That last detail is the point of the file. "Without closing or replacing the
 * child tab" is a claim about what is still there *after* the click, and a test
 * that never opened the child tab could not have noticed it going.
 */
import { scopeThreadRef } from "@t3tools/client-runtime/environment";
import { type EnvironmentId, ThreadId } from "@t3tools/contracts";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";

const view = vi.hoisted(() => ({
  current: {
    data: null as unknown,
    error: null as string | null,
    isPending: false,
    refresh: () => {},
  },
}));

vi.mock("../../state/subagents", () => ({
  subagentEnvironment: { stream: () => Symbol("subagent-stream-atom") },
}));
vi.mock("../../state/query", () => ({
  useEnvironmentQuery: () => view.current,
}));

import { selectThreadRightPanelState, useRightPanelStore } from "../../rightPanelStore";
import { SubagentStreamPanel } from "./SubagentStreamPanel";

const threadRef = scopeThreadRef("env-1" as EnvironmentId, ThreadId.make("thread-A"));

const surfaces = () =>
  selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, threadRef).surfaces.map(
    (surface) => surface.id,
  );

function entry(id: string, kind: "read" | "edit", path: string) {
  return {
    id,
    sequence: 1,
    kind,
    payload: {
      title: path,
      status: "completed" as const,
      detail: null,
      command: null,
      paths: [path],
      query: null,
    },
    createdAt: "2026-08-17T00:00:01.000Z",
  };
}

function renderWork(entries: ReadonlyArray<unknown>, workspaceRoot?: string) {
  view.current = {
    data: {
      stream: {
        childId: "call_task_1",
        parentChildId: null,
        name: "explore",
        assignment: "Count the files",
        state: "working",
        outcome: null,
        entryCount: entries.length,
        createdAt: "2026-08-17T00:00:00.000Z",
        updatedAt: "2026-08-17T00:00:01.000Z",
      },
      entries,
    },
    error: null,
    isPending: false,
    refresh: () => {},
  };
  return render(
    <SubagentStreamPanel
      environmentId={"environment-local" as EnvironmentId}
      threadId={ThreadId.make("thread-A")}
      childId="call_task_1"
      threadRef={threadRef}
      workspaceRoot={workspaceRoot}
    />,
  );
}

beforeEach(() => {
  useRightPanelStore.setState({ byThreadKey: {} });
  // The developer got here by clicking the child's inline row, so its tab is
  // open and active before anything below happens.
  useRightPanelStore.getState().openSubagent(threadRef, "call_task_1");
});

afterEach(() => {
  cleanup();
});

describe("opening an artifact from inside a child's work", () => {
  it("opens a file the child read beside the child's own tab", () => {
    renderWork([entry("r", "read", "src/main.rs")]);

    fireEvent.click(screen.getByTitle("src/main.rs"));

    expect(surfaces()).toEqual(["subagent:call_task_1", "file:src/main.rs"]);
  });

  it("opens the diff from a child's edit beside the child's own tab", () => {
    renderWork([entry("e", "edit", "src/counted.rs")]);

    fireEvent.click(screen.getByText("Open diff"));

    expect(surfaces()).toEqual(["subagent:call_task_1", "diff"]);
  });

  /**
   * OpenCode reports the path the model wrote, and for `read`/`edit` that is
   * absolute. The tab has to open on the file the child touched, not on a
   * surface addressed by a path the panel cannot resolve.
   */
  it("opens an absolute path the child reported as the workspace file it names", () => {
    renderWork([entry("r", "read", "/home/dev/project/src/main.rs")], "/home/dev/project");

    fireEvent.click(screen.getByTitle("/home/dev/project/src/main.rs"));

    expect(surfaces()).toEqual(["subagent:call_task_1", "file:src/main.rs"]);
  });

  /**
   * And a file outside the workspace has no surface in this window, so the
   * child's entry is still drawn — it happened — with nothing to click.
   */
  it("offers no link for a file outside the workspace", () => {
    renderWork([entry("r", "read", "/etc/hosts")], "/home/dev/project");

    expect(screen.queryByTitle("/etc/hosts")).toBeNull();
    expect(surfaces()).toEqual(["subagent:call_task_1"]);
  });
});
