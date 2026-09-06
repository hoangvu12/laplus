// @vitest-environment happy-dom
import { EventId, TurnId } from "@t3tools/contracts";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vite-plus/test";

import { deriveWorkLogEntries } from "../../session-logic";
import { SimpleWorkEntryRow } from "./MessagesTimeline";

afterEach(cleanup);

describe("expanded tool results", () => {
  it.each([
    "Complete output kept beyond the short preview",
    { files: [{ path: "src/main.ts", content: "Complete output kept beyond the short preview" }] },
  ])("shows a read result without turning its paths into edits", (result) => {
    const [entry] = deriveWorkLogEntries([
      {
        id: EventId.make("read-done"),
        kind: "tool.completed",
        tone: "tool",
        summary: "Read",
        turnId: TurnId.make("turn-1"),
        createdAt: "2026-09-06T00:00:00.000Z",
        payload: {
          itemType: "dynamic_tool_call",
          title: "Read",
          detail: "src/main.ts",
          data: {
            toolCallId: "read-1",
            toolName: "read_file",
            input: { paths: ["src/main.ts"] },
            result,
          },
        },
      },
    ]);
    if (!entry) throw new Error("Expected a read entry");
    const { container } = render(
      <SimpleWorkEntryRow workEntry={entry} workspaceRoot={undefined} />,
    );

    expect(container.textContent).not.toContain("Complete output kept beyond the short preview");
    fireEvent.click(screen.getByRole("button", { name: "Read - src/main.ts" }));
    expect(container.textContent).toContain("Complete output kept beyond the short preview");
    expect(entry.changedFiles).toBeUndefined();
    expect(container.querySelector(".lucide-square-pen")).toBeNull();
  });
});
