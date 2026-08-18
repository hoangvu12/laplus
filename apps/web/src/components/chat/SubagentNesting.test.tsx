// @vitest-environment happy-dom
/**
 * A nested child, in the one place it is shown.
 *
 * Spec stories 44 and 45: a subagent a child launched appears **inside that
 * child's work stream**, and clicking it opens the descendant as another
 * ordinary right-panel tab. Both halves are driven here rather than asserted
 * about — the real `SubagentStreamPanel`, a real click, and the real
 * right-panel store — because the claim is a link between two surfaces and
 * either end of it can be right while the link is broken.
 *
 * Story 46's other half, that a descendant is *not* also drawn in the root
 * transcript, is the server's and is proven where the transcript is:
 * `socket_codex_turn::codex_children_of_one_collaboration_keep_separate_identities_and_endings`.
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

// The stream is stubbed for the reason `SubagentStreamPanel.test.tsx` stubs it:
// the atom family opens a socket subscription, and what this file is about is
// what a reader can click.
vi.mock("../../state/subagents", () => ({
  subagentEnvironment: { stream: () => Symbol("subagent-stream-atom") },
}));
vi.mock("../../state/query", () => ({
  useEnvironmentQuery: () => view.current,
}));

import { selectThreadRightPanelState, useRightPanelStore } from "../../rightPanelStore";
import { SubagentStreamPanel } from "./SubagentStreamPanel";

const ENVIRONMENT_ID = "env-1" as EnvironmentId;
const THREAD_ID = ThreadId.make("thread-A");
const threadRef = scopeThreadRef(ENVIRONMENT_ID, THREAD_ID);

const surfaces = () =>
  selectThreadRightPanelState(useRightPanelStore.getState().byThreadKey, threadRef);

function head(overrides: Record<string, unknown> = {}) {
  return {
    childId: "reviewer",
    parentChildId: null,
    name: "reviewer",
    assignment: "Review the decoder",
    state: "working",
    outcome: null,
    entryCount: 0,
    createdAt: "2026-08-17T00:00:00.000Z",
    updatedAt: "2026-08-17T00:00:01.000Z",
    ...overrides,
  };
}

function launcher(payload: Record<string, unknown>) {
  return {
    id: "reviewer:k:child:helper",
    sequence: 2,
    kind: "subagent" as const,
    payload: {
      childId: "helper",
      name: "helper",
      assignment: "Check the tests",
      state: "working",
      outcome: null,
      ...payload,
    },
    createdAt: "2026-08-17T00:00:02.000Z",
  };
}

function open(entries: ReadonlyArray<unknown>) {
  view.current = {
    data: { stream: head(), entries },
    error: null,
    isPending: false,
    refresh: () => {},
  };
  return render(
    <SubagentStreamPanel
      environmentId={ENVIRONMENT_ID}
      threadId={THREAD_ID}
      childId="reviewer"
      threadRef={threadRef}
    />,
  );
}

beforeEach(() => {
  useRightPanelStore.setState({ byThreadKey: {} });
  view.current = { data: null, error: null, isPending: false, refresh: () => {} };
});

afterEach(() => {
  cleanup();
});

describe("a nested child inside its parent's work stream", () => {
  it("opens the descendant as another ordinary child tab", () => {
    open([launcher({})]);

    fireEvent.click(screen.getByRole("button", { name: /Open subagent work stream/ }));

    const state = surfaces();
    expect(state.surfaces).toEqual([
      { id: "subagent:helper", kind: "subagent", resourceId: "helper" },
    ]);
    expect(state.activeSurfaceId).toBe("subagent:helper");
    expect(state.isOpen).toBe(true);
  });

  /**
   * The same open-versus-activate rule the inline row follows, because it is
   * the same store operation: a second click on a descendant already open
   * brings its tab forward rather than adding a second.
   */
  it("activates the tab it already has rather than duplicating it", () => {
    open([launcher({})]);
    const control = screen.getByRole("button", { name: /Open subagent work stream/ });

    fireEvent.click(control);
    fireEvent.click(control);

    expect(surfaces().surfaces).toHaveLength(1);
  });

  /**
   * The launcher is the descendant's compact row, so it has to carry what the
   * inline row carries: who it is and where it got to.
   */
  it("names the descendant and says where it got to", () => {
    open([
      launcher({
        state: "completed",
        outcome: { kind: "completed", text: "eleven tests pass" },
      }),
    ]);

    const row = screen.getByRole("button", { name: /Open subagent work stream/ });
    expect(row.textContent).toContain("Subagent helper");
    expect(row.textContent).toContain("eleven tests pass");
  });

  /**
   * The case the test above cannot see: a descendant that ended **without**
   * saying anything. Its `outcome.text` is null, and the row used to fall
   * through to `assignment` — so an interrupted worker showed the task it had
   * been given as though that were what came back, which is the stale activity
   * the spec's terminal rule exists to displace ("replace latest activity
   * atomically with a bounded result, failure, interruption, or empty-result
   * preview").
   *
   * Asserted for all three silent endings, and each asserts the *absence* of the
   * assignment as well as the presence of the sentence — the bug was a fallback,
   * so a test that only looked for the right words would have passed while the
   * wrong ones sat beside them.
   */
  it.each([
    ["interrupted", { kind: "interrupted" as const, text: null }, "Interrupted"],
    ["failed with no reason", { kind: "failed" as const, text: null }, "Failed"],
    [
      "completed with nothing",
      { kind: "empty" as const, text: null },
      "Completed — no result returned",
    ],
  ])("says what came back when a descendant ends silently (%s)", (_name, outcome, expected) => {
    open([launcher({ state: outcome.kind === "empty" ? "completed" : outcome.kind, outcome })]);

    const row = screen.getByRole("button", { name: /Open subagent work stream/ });
    expect(row.textContent).toContain(expected);
    expect(row.textContent).not.toContain("Check the tests");
  });

  /**
   * The negative claim, and the one that keeps the surface read-only: a stream
   * with no descendant offers nothing to launch. Without this the test above
   * would pass against a panel that drew a launcher for every entry.
   */
  it("offers nothing to launch when the child delegated nothing", () => {
    open([
      {
        id: "reviewer:k:item:1",
        sequence: 1,
        kind: "message" as const,
        payload: { text: "reading the decoder" },
        createdAt: "2026-08-17T00:00:01.000Z",
      },
    ]);

    expect(screen.queryByRole("button", { name: /Open subagent work stream/ })).toBeNull();
  });
});
