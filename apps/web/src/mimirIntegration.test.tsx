import { renderToStaticMarkup } from "react-dom/server";
import { Schema } from "effect";
import { describe, expect, it } from "vite-plus/test";
import {
  EventId,
  ProviderDriverKind,
  ProviderInstanceId,
  ServerProvider,
} from "@t3tools/contracts";
import { getProviderInteractionModeToggle } from "./providerModels";
import { derivePendingUserInputs } from "./session-logic";
import { buildPendingUserInputAnswers } from "./pendingUserInput";
import { getDriverOption } from "./components/settings/providerDriverMeta";
import { ProviderSettingsForm } from "./components/settings/ProviderSettingsForm";

const mimir = ProviderDriverKind.make("mimir");
const decodeProvider = Schema.decodeUnknownSync(ServerProvider);

describe("Mimir native integration", () => {
  it("renders bridge configuration and leaves credentials in Mimir", () => {
    const definition = getDriverOption(mimir);
    expect(definition).toBeDefined();
    const html = renderToStaticMarkup(
      <ProviderSettingsForm
        definition={definition!}
        value={{}}
        idPrefix="mimir"
        variant="card"
        onChange={() => {}}
      />,
    );
    expect(html).toContain("Binary path");
    expect(html).toContain("Bridge command");
    expect(html).toContain("/org.mimir.bridge:serve");
    expect(html).toContain("credentials in Mimir");
    expect(html).not.toContain("Launch arguments");
  });

  it("uses the selected instance's interaction controls, not its driver's default", () => {
    const providers = [
      decodeProvider({
        instanceId: "mimir",
        driver: "mimir",
        displayName: "Mimir",
        enabled: true,
        installed: true,
        version: null,
        auth: { status: "unknown" },
        checkedAt: "2026-09-06T00:00:00.000Z",
        status: "ready",
        models: [],
        showInteractionModeToggle: true,
      }),
      decodeProvider({
        instanceId: "mimir_work",
        driver: "mimir",
        displayName: "Work",
        enabled: true,
        installed: true,
        version: null,
        auth: { status: "unknown" },
        checkedAt: "2026-09-06T00:00:00.000Z",
        status: "ready",
        models: [],
        showInteractionModeToggle: false,
      }),
    ];
    expect(getProviderInteractionModeToggle(providers, ProviderInstanceId.make("mimir_work"))).toBe(
      false,
    );
  });

  it("keeps free-text questions and their host-assigned answer IDs", () => {
    const pending = derivePendingUserInputs([
      {
        id: EventId.make("question"),
        createdAt: "2026-09-06T00:00:00.000Z",
        kind: "user-input.requested",
        tone: "info",
        summary: "Question",
        turnId: null,
        payload: {
          requestId: "host-answer-42",
          questions: [
            { id: "target", header: "Target", question: "Which directory?", options: [] },
          ],
        },
      },
    ]);
    expect(pending).toHaveLength(1);
    expect(pending[0]?.requestId).toBe("host-answer-42");
    expect(pending[0]?.questions[0]?.options).toEqual([]);
    expect(
      buildPendingUserInputAnswers(pending[0]!.questions, {
        target: { selectedOptionLabels: [], customAnswer: "/workspace" },
      }),
    ).toEqual({ target: "/workspace" });
  });
});
