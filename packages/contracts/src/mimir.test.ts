import { Schema } from "effect";
import { describe, expect, it } from "vite-plus/test";
import { ClientOrchestrationCommand, OrchestrationProposedPlan } from "./orchestration.ts";
import { MimirSettings, ServerSettings } from "./settings.ts";
import { ProviderInstanceId } from "./providerInstance.ts";

const decodeCommand = Schema.decodeUnknownSync(ClientOrchestrationCommand);
const decodeMimirSettings = Schema.decodeUnknownSync(MimirSettings);
const decodeSettings = Schema.decodeUnknownSync(ServerSettings);
const decodePlan = Schema.decodeUnknownSync(OrchestrationProposedPlan);

describe("native Mimir contracts", () => {
  it("defaults bridge settings without adding a legacy provider bucket", () => {
    expect(decodeMimirSettings({})).toEqual({
      binaryPath: "mimir",
      bridgeCommand: "/org.mimir.bridge:serve",
    });
    const settings = decodeSettings({
      providerInstances: {
        mimir_work: {
          driver: "mimir",
          displayName: "Mimir Work",
          enabled: true,
          config: { binaryPath: "/opt/mimir" },
        },
      },
    });
    expect(settings.providers).not.toHaveProperty("mimir");
    expect(settings.providerInstances[ProviderInstanceId.make("mimir_work")]?.config).toEqual({
      binaryPath: "/opt/mimir",
    });
  });

  it.each(["implement", "save-and-stop"])(
    "preserves the actual saved plan ID for %s",
    (decision) => {
      const command = {
        type: "thread.plan.decide",
        commandId: "decision-1",
        threadId: "thread-1",
        planId: "sdk-plan-42",
        decision,
      };
      expect(decodeCommand(command)).toEqual(command);
      const plan = decodePlan({
        id: "sdk-plan-42",
        turnId: null,
        planMarkdown: "# Plan",
        decision,
        createdAt: "2026-09-06T00:00:00.000Z",
        updatedAt: "2026-09-06T00:00:00.000Z",
      });
      expect(plan.decision).toBe(decision);
    },
  );

  it("rejects missing plan identity or unsupported decisions", () => {
    expect(() =>
      decodeCommand({
        type: "thread.plan.decide",
        commandId: "c",
        threadId: "t",
        decision: "implement",
      }),
    ).toThrow();
    expect(() =>
      decodeCommand({
        type: "thread.plan.decide",
        commandId: "c",
        threadId: "t",
        planId: "p",
        decision: "approve",
      }),
    ).toThrow();
  });

  it("validates active steering separately from normal turn start", () => {
    const command = {
      type: "thread.turn.steer",
      commandId: "c",
      threadId: "t",
      text: "Focus on tests",
    };
    expect(decodeCommand(command)).toEqual(command);
    expect(() => decodeCommand({ ...command, text: " " })).toThrow();
  });
});
