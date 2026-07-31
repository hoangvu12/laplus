import { ApprovalRequestId } from "@t3tools/contracts";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";

import { ComposerPendingApprovalActions } from "./ComposerPendingApprovalActions";

describe("ComposerPendingApprovalActions", () => {
  it("renders exactly the decisions the request offers", () => {
    const markup = renderToStaticMarkup(
      <ComposerPendingApprovalActions
        requestId={ApprovalRequestId.make("codex:0")}
        availableDecisions={["accept", "cancel"]}
        isResponding={false}
        onRespondToApproval={async () => undefined}
      />,
    );

    expect(markup).toContain("Approve once");
    expect(markup).toContain("Cancel turn");
    expect(markup).not.toContain("Decline");
    expect(markup).not.toContain("Always allow this session");
  });

  it("keeps all four actions for activities without a request-specific list", () => {
    const markup = renderToStaticMarkup(
      <ComposerPendingApprovalActions
        requestId={ApprovalRequestId.make("claude-request")}
        isResponding={false}
        onRespondToApproval={async () => undefined}
      />,
    );

    for (const label of ["Approve once", "Always allow this session", "Decline", "Cancel turn"]) {
      expect(markup).toContain(label);
    }
  });
});
