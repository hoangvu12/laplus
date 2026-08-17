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
import { openSubagentDiff, openSubagentFile, subagentFileTarget } from "./subagentFileActions";

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

/**
 * The address a child's path has to be turned into before the file surface can
 * be opened on it.
 *
 * A provider reports the path *it* used, and for OpenCode's `read` and `edit`
 * that is what the model wrote — usually absolute. The right panel's file
 * surface is addressed by a workspace-relative path, so handing the raw one
 * through opens a tab on a file the panel cannot resolve: a surface that
 * appears and is empty, which is worse than offering nothing.
 */
describe("addressing a file a child reported", () => {
  const root = "/home/dev/project";

  it("makes an absolute path inside the workspace relative", () => {
    expect(subagentFileTarget(`${root}/src/main.rs`, root)).toBe("src/main.rs");
  });

  it("passes an already-relative path through", () => {
    expect(subagentFileTarget("src/main.rs", root)).toBe("src/main.rs");
    expect(subagentFileTarget("src/main.rs", undefined)).toBe("src/main.rs");
  });

  /**
   * A child may legitimately read a file outside the workspace, and this window
   * has no surface for one. `null` is what the caller renders no affordance
   * from — the honest answer, rather than a link that opens nothing.
   */
  it("refuses a path it cannot address", () => {
    expect(subagentFileTarget("/etc/hosts", root)).toBeNull();
    expect(subagentFileTarget("/home/dev/other/src/main.rs", root)).toBeNull();
    expect(subagentFileTarget("/home/dev/project/src/main.rs", undefined)).toBeNull();
  });

  it("opens the workspace-relative address rather than the reported path", () => {
    const target = subagentFileTarget(`${root}/src/main.rs`, root);
    expect(target).not.toBeNull();
    openSubagentFile(ref, target!);

    expect(surfaces()).toEqual(["subagent:call_task_1", "file:src/main.rs"]);
  });
});
