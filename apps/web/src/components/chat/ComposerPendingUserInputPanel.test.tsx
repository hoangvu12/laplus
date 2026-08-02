import { ApprovalRequestId } from "@t3tools/contracts";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";

import { ComposerPendingUserInputPanel } from "./ComposerPendingUserInputPanel";

describe("ComposerPendingUserInputPanel", () => {
  it("offers an explicit rejection action for a pending question", () => {
    const markup = renderToStaticMarkup(
      <ComposerPendingUserInputPanel
        pendingUserInputs={[
          {
            requestId: ApprovalRequestId.make("question-1"),
            createdAt: "2026-08-02T00:00:00.000Z",
            questions: [
              {
                id: "question-0-database",
                header: "Database",
                question: "Which database?",
                options: [{ label: "SQLite", description: "Local" }],
                multiSelect: false,
              },
            ],
          },
        ]}
        respondingRequestIds={[]}
        answers={{}}
        questionIndex={0}
        onToggleOption={() => undefined}
        onAdvance={() => undefined}
        onReject={() => undefined}
      />,
    );

    expect(markup).toContain(">Reject</button>");
  });
});
