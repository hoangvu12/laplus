import { ApprovalRequestId } from "@t3tools/contracts";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";

import { ComposerPendingApprovalPanel } from "./ComposerPendingApprovalPanel";

describe("ComposerPendingApprovalPanel", () => {
  it("renders complete multiline command details without hover or truncation", () => {
    const detail = `bun run release -- ${"long-argument ".repeat(20)}\nsecond line`;
    const markup = renderToStaticMarkup(
      <ComposerPendingApprovalPanel
        approval={{
          requestId: ApprovalRequestId.make("approval-1"),
          requestKind: "command",
          createdAt: "2026-07-18T00:00:00.000Z",
          detail,
        }}
        pendingCount={1}
      />,
    );

    expect(markup).toContain('data-approval-detail="complete"');
    expect(markup).toContain('aria-label="Command"');
    expect(markup).toContain(detail);
    expect(markup).not.toContain("truncate");
    expect(markup).not.toContain("line-clamp");
  });

  /**
   * A blocker a delegated child raised is answered here, in the main
   * conversation, wherever the developer happens to be looking — so the panel
   * has to say whose work the decision will unblock. Approving a command a
   * worker asked for is not the same act as approving one the agent you are
   * talking to asked for.
   */
  it("names the subagent waiting on a decision", () => {
    const markup = renderToStaticMarkup(
      <ComposerPendingApprovalPanel
        approval={{
          requestId: ApprovalRequestId.make("child-per-1"),
          requestKind: "command",
          createdAt: "2026-08-17T00:00:00.000Z",
          detail: "rm -rf build",
          subagent: { childId: "call_task_1", name: "explore" },
        }}
        pendingCount={1}
      />,
    );

    expect(markup).toContain("from subagent explore");
    expect(markup).toContain('data-waiting-subagent="call_task_1"');
  });

  /** A request the root agent raised names nobody, which is the truth. */
  it("says nothing about a subagent when the root agent asked", () => {
    const markup = renderToStaticMarkup(
      <ComposerPendingApprovalPanel
        approval={{
          requestId: ApprovalRequestId.make("approval-2"),
          requestKind: "command",
          createdAt: "2026-08-17T00:00:00.000Z",
          detail: "ls",
        }}
        pendingCount={1}
      />,
    );

    expect(markup).not.toContain("subagent");
  });
});
