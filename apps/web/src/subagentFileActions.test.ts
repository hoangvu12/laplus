/**
 * What a file or diff action inside a child's work stream does to the
 * workspace.
 *
 * The claim is one word — *neighbouring* — and it is a claim about the store
 * rather than about the markup, so it is proved against the real store: opening
 * a file or the diff from a child's work adds a tab beside the child's own and
 * leaves that child's tab exactly where it was, open and reachable. A developer
 * who inspects an artifact and cannot get back to the worker that produced it
 * has lost the thing they clicked into.
 */
import { scopeThreadRef } from "@t3tools/client-runtime/environment";
import { type EnvironmentId, ThreadId } from "@t3tools/contracts";
import { beforeEach, describe, expect, it } from "vite-plus/test";

import { selectThreadRightPanelState, useRightPanelStore } from "./rightPanelStore";
import { openSubagentDiff, openSubagentFile } from "./subagentFileActions";

const ref = scopeThreadRef("env-1" as EnvironmentId, ThreadId.make("thread-A"));

const surfaces = () =>
  selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, ref).surfaces.map(
    (surface) => surface.id,
  );
const active = () =>
  selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, ref).activeSurfaceId;

beforeEach(() => {
  useRightPanelStore.setState({ byThreadKey: {} });
  useRightPanelStore.getState().openSubagent(ref, "call_task_1");
});

describe("file and diff actions from a child's work stream", () => {
  it("opens the file beside the child's tab rather than in place of it", () => {
    openSubagentFile(ref, "src/main.rs");

    expect(surfaces()).toEqual(["subagent:call_task_1", "file:src/main.rs"]);
    expect(active()).toBe("file:src/main.rs");
  });

  it("opens the diff beside the child's tab rather than in place of it", () => {
    openSubagentDiff(ref);

    expect(surfaces()).toEqual(["subagent:call_task_1", "diff"]);
    expect(active()).toBe("diff");
  });

  /**
   * Several artifacts from one child, and every one of them still leaves the
   * child there — including the second file, which is where a surface that
   * reused one slot would show itself.
   */
  it("keeps the child reachable through a whole inspection", () => {
    openSubagentFile(ref, "src/main.rs");
    openSubagentFile(ref, "src/counted.rs");
    openSubagentDiff(ref);

    expect(surfaces()).toContain("subagent:call_task_1");
    expect(surfaces()).toEqual([
      "subagent:call_task_1",
      "file:src/main.rs",
      "file:src/counted.rs",
      "diff",
    ]);

    // And clicking back onto the child is an activation, not a reopen.
    useRightPanelStore.getState().activateSurface(ref, "subagent:call_task_1");
    expect(active()).toBe("subagent:call_task_1");
    expect(surfaces()).toHaveLength(4);
  });
});
