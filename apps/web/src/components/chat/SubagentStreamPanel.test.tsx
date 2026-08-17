/**
 * The child surface, judged by what it puts on the screen.
 *
 * Every assertion here is about semantic output rather than component
 * structure: that the child's prose goes through the main agent's markdown
 * renderer rather than a reduced log, that each terminal outcome is named,
 * and — the two negative claims the ticket actually turns on — that the surface
 * carries no composer and no identity/task header even though the stream it is
 * given carries a name and an assignment to build one from.
 *
 * Ticket 01 records two entry kinds, `message` and `outcome`, so representative
 * coverage is those plus the four outcome shapes. Commands, reads, edits, tool
 * calls and blockers are ticket 02's, and its worker extends this file.
 */
import { scopeThreadRef } from "@t3tools/client-runtime/environment";
import type { EnvironmentId, ThreadId } from "@t3tools/contracts";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vite-plus/test";

const view = vi.hoisted(() => ({
  current: {
    data: null as unknown,
    error: null as string | null,
    isPending: false,
    refresh: () => {},
  },
}));

// The atom family is stubbed rather than driven: it opens a socket
// subscription against a live environment, and what this file is about is the
// markup, not the transport. `subagentStream.test.ts` covers the folding.
vi.mock("../../state/subagents", () => ({
  subagentEnvironment: { stream: () => Symbol("subagent-stream-atom") },
}));
vi.mock("../../state/query", () => ({
  useEnvironmentQuery: () => view.current,
}));

import { SubagentStreamPanel } from "./SubagentStreamPanel";
import { SCROLL_TO_END_LABEL } from "./ScrollToEndButton";
import {
  rememberSubagentScroll,
  resetSubagentScrollMemoryForTests,
  subagentScrollKey,
} from "./subagentScroll";

const ENVIRONMENT_ID = "environment-local" as EnvironmentId;
const THREAD_ID = "thread-1" as ThreadId;
const SURFACE_KEY = subagentScrollKey(scopeThreadRef(ENVIRONMENT_ID, THREAD_ID), "call_task_1");

/**
 * Every control the surface renders, by the name a reader would hear. An
 * unlabelled button comes back as its raw markup rather than being skipped, so
 * this cannot quietly under-report what the surface offers.
 */
function controlsIn(markup: string): string[] {
  return Array.from(markup.matchAll(/<button\b[^>]*>/g)).map(
    (match) => /aria-label="([^"]*)"/.exec(match[0])?.[1] ?? match[0],
  );
}

const CHILD_NAME = "explore";
const CHILD_ASSIGNMENT = "Count the files in the workspace";

function stream(overrides: Record<string, unknown> = {}) {
  return {
    childId: "call_task_1",
    parentChildId: null,
    // Carried deliberately: the surface is given an identity and an assignment
    // and must still not draw a header out of them.
    name: CHILD_NAME,
    assignment: CHILD_ASSIGNMENT,
    state: "working",
    outcome: null,
    entryCount: 0,
    createdAt: "2026-08-17T00:00:00.000Z",
    updatedAt: "2026-08-17T00:00:01.000Z",
    ...overrides,
  };
}

function message(id: string, sequence: number, text: string) {
  return {
    id,
    sequence,
    kind: "message" as const,
    payload: { text },
    createdAt: "2026-08-17T00:00:01.000Z",
  };
}

function outcome(kind: string, text: string | null) {
  return {
    id: "call_task_1:k:outcome",
    sequence: 9,
    kind: "outcome" as const,
    payload: { kind, text },
    createdAt: "2026-08-17T00:00:09.000Z",
  };
}

function render(
  data: { stream: ReturnType<typeof stream> | null; entries: ReadonlyArray<unknown> } | null,
  error: string | null = null,
): string {
  view.current = { data, error, isPending: false, refresh: () => {} };
  return renderToStaticMarkup(
    <SubagentStreamPanel
      environmentId={ENVIRONMENT_ID}
      threadId={THREAD_ID}
      childId="call_task_1"
    />,
  );
}

describe("the subagent work stream surface", () => {
  beforeEach(() => {
    view.current = { data: null, error: null, isPending: false, refresh: () => {} };
    resetSubagentScrollMemoryForTests();
  });

  it("renders the child's prose through the main agent's markdown language", () => {
    const markup = render({
      stream: stream(),
      entries: [
        message("a", 1, "Looking through **src/** for the count"),
        message("b", 2, "- eleven files\n- two directories"),
      ],
    });

    // Markdown, not a raw log: the emphasis and the list are rendered.
    expect(markup).toContain("<strong>src/</strong>");
    expect(markup).toContain("<li>");
    expect(markup).toContain("eleven files");
    expect(markup).toContain("two directories");
  });

  /**
   * The conclusion is the terminal entry of the same stream, drawn as a
   * work-entry row rather than as more prose, so it reads as an answer.
   */
  it("names a completed child's result and renders it as markdown", () => {
    const markup = render({
      stream: stream({ state: "completed", outcome: { kind: "completed", text: "eleven files" } }),
      entries: [message("a", 1, "counting"), outcome("completed", "**eleven** files")],
    });

    expect(markup).toContain("Result");
    expect(markup).toContain("<strong>eleven</strong>");
    expect(markup).toContain('data-subagent-entry="outcome"');
  });

  /**
   * The conclusion goes through the main agent's own work-entry row, not a
   * renderer this surface owns. The failure indicator is the shared row's and
   * nothing here could produce it, so its presence is what says the two agree.
   */
  it("draws the conclusion with the main agent's work-entry language", () => {
    const markup = render({
      stream: stream({ state: "failed", outcome: { kind: "failed", text: "the tool exited 1" } }),
      entries: [outcome("failed", "the tool exited 1")],
    });

    expect(markup).toContain('aria-label="Tool call failed"');
  });

  /**
   * A child that finished and returned nothing has concluded. The surface has
   * to say so, or an empty tail reads as a stream that broke off.
   */
  it("says a silent completion is a completion rather than a gap", () => {
    const markup = render({
      stream: stream({ state: "completed", outcome: { kind: "empty", text: null } }),
      entries: [message("a", 1, "counting"), outcome("empty", null)],
    });

    expect(markup).toContain("Completed — no result returned");
  });

  it("names a failure and an interruption distinctly", () => {
    const failed = render({
      stream: stream({ state: "failed", outcome: { kind: "failed", text: "the tool exited 1" } }),
      entries: [outcome("failed", "the tool exited 1")],
    });
    expect(failed).toContain("Failed");
    expect(failed).toContain("the tool exited 1");

    const interrupted = render({
      stream: stream({ state: "interrupted", outcome: { kind: "interrupted", text: null } }),
      entries: [outcome("interrupted", null)],
    });
    expect(interrupted).toContain("Interrupted");
    expect(interrupted).not.toContain("Failed");
  });

  /**
   * The two negative claims. The surface is observational: it cannot steer a
   * provider that has no way to receive a message for one child, so it offers
   * nothing that looks like it could.
   *
   * The claim is about the *complete* set of controls, and is stated in both
   * follow states so that neither passes by accident. A surface opened at its
   * live edge is following and renders no control at all; the test below is the
   * same surface with following suspended, where the only control it can ever
   * render appears — the shared jump-to-latest affordance, which moves the
   * viewport and reaches no provider.
   */
  it("offers no composer and no way to send anything to the child", () => {
    const markup = render({
      stream: stream(),
      entries: [message("a", 1, "counting"), outcome("completed", "eleven files")],
    });

    expect(markup).not.toContain("<textarea");
    expect(markup).not.toContain("<input");
    expect(markup).not.toContain("<form");
    expect(markup).not.toContain("contenteditable");
    // Nothing in the surface is activatable either: the work-entry rows it
    // reuses are inert here, where in the conversation they expand or launch.
    expect(markup).not.toContain('role="button"');
    expect(controlsIn(markup)).toEqual([]);
  });

  /**
   * The same surface with its reader scrolled away from the live edge. This is
   * the state in which the surface does render a control, so it is where the
   * "no composer" claim above has to be made again to mean anything.
   */
  it("renders only the shared jump-to-latest control once following is suspended", () => {
    rememberSubagentScroll(SURFACE_KEY, { offset: 120, following: false });

    const markup = render({
      stream: stream(),
      entries: [message("a", 1, "counting"), outcome("completed", "eleven files")],
    });

    expect(controlsIn(markup)).toEqual([SCROLL_TO_END_LABEL]);
    expect(markup).not.toContain("<textarea");
    expect(markup).not.toContain("<input");
    expect(markup).not.toContain("<form");
    expect(markup).not.toContain("contenteditable");
  });

  /**
   * The inline row in the conversation is where identity, assignment and state
   * are reported. Repeating them here would spend panel space on what the
   * developer just clicked.
   */
  it("opens directly into the work with no identity or task header", () => {
    const markup = render({
      stream: stream(),
      entries: [message("a", 1, "counting")],
    });

    expect(markup).not.toContain(CHILD_NAME);
    expect(markup).not.toContain(CHILD_ASSIGNMENT);
    expect(markup).not.toContain("call_task_1");
    expect(markup).not.toMatch(/<h[1-6][ >]/);
  });

  it("distinguishes loading, a child that has done nothing, and one that is gone", () => {
    expect(render(null)).toContain('data-subagent-state="loading"');
    expect(render({ stream: stream({ state: "pending" }), entries: [] })).toContain(
      'data-subagent-state="empty"',
    );
    expect(render(null, "Subagent call_task_1 was not found in thread thread-1")).toContain(
      'data-subagent-state="unavailable"',
    );
  });
});
