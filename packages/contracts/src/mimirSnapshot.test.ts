import { Schema } from "effect";
import { describe, expect, it } from "vite-plus/test";

import { OrchestrationThreadStreamItem } from "./orchestration.ts";

const decodeThreadStreamItem = Schema.decodeUnknownSync(OrchestrationThreadStreamItem);
const timestamp = "2026-09-06T00:00:00.000Z";

// Reduced from a native Save-and-stop/reload wire capture: keep the completed
// turn, saved SDK plan, checkpoint, message and Mimir session in one snapshot.
// IDs, timestamps and file names are synthetic; no local paths or auth state.
// This verifies the contract, not that the Rust serializer emits this shape.
function savedPlanSnapshot() {
  return {
    kind: "snapshot",
    snapshot: {
      snapshotSequence: 34,
      thread: {
        id: "thread-1",
        projectId: "project-1",
        title: "Plan integration",
        modelSelection: { instanceId: "mimir", model: "process-test/offline" },
        runtimeMode: "full-access",
        interactionMode: "default",
        branch: null,
        worktreePath: null,
        createdAt: timestamp,
        updatedAt: timestamp,
        deletedAt: null,
        latestTurn: {
          turnId: "turn-1",
          state: "completed",
          requestedAt: timestamp,
          startedAt: timestamp,
          completedAt: timestamp,
          assistantMessageId: null,
        },
        messages: [
          {
            id: "message-1",
            role: "user",
            text: "Plan integration",
            turnId: "turn-1",
            streaming: false,
            createdAt: timestamp,
            updatedAt: timestamp,
          },
        ],
        proposedPlans: [
          {
            id: "sdk-plan-1",
            turnId: "turn-1",
            planMarkdown: "# Requirements\n\nVerify native Mimir workflows.",
            decision: "save-and-stop",
            status: "saved-stopped",
            implementedAt: null,
            implementationThreadId: null,
            createdAt: timestamp,
            updatedAt: timestamp,
          },
        ],
        activities: [],
        checkpoints: [
          {
            turnId: "turn-1",
            checkpointTurnCount: 1,
            checkpointRef: "refs/laplus/checkpoints/thread-1/turn/1",
            status: "ready",
            files: [{ path: "plan.md", kind: "added", additions: 3, deletions: 0 }],
            assistantMessageId: null,
            completedAt: timestamp,
          },
        ],
        session: {
          threadId: "thread-1",
          status: "ready",
          providerName: "mimir",
          providerInstanceId: "mimir",
          runtimeMode: "full-access",
          activeTurnId: null,
          lastError: null,
          updatedAt: timestamp,
        },
      },
    },
  };
}

describe("native Mimir thread snapshot wire contract", () => {
  it("decodes the whole saved-plan reload snapshot with an absent source and ready checkpoint", () => {
    const wire = savedPlanSnapshot();
    const decoded = decodeThreadStreamItem(wire);
    expect(decoded).toMatchObject(wire);
    if (decoded.kind !== "snapshot") throw new Error("Expected a thread snapshot");
    expect(decoded.snapshot.thread.latestTurn).not.toHaveProperty("sourceProposedPlan");
    expect(decoded.snapshot.thread.latestTurn?.state).toBe("completed");
    expect(decoded.snapshot.thread.checkpoints[0]?.status).toBe("ready");
    expect(decoded.snapshot.thread.proposedPlans[0]).toMatchObject({
      id: "sdk-plan-1",
      decision: "save-and-stop",
      status: "saved-stopped",
    });
  });

  it("preserves a real source plan reference when present", () => {
    const wire = savedPlanSnapshot();
    const sourceProposedPlan = { threadId: "source-thread", planId: "source-plan" };
    Object.assign(wire.snapshot.thread.latestTurn, { sourceProposedPlan });
    const decoded = decodeThreadStreamItem(wire);
    if (decoded.kind !== "snapshot") throw new Error("Expected a thread snapshot");
    expect(decoded.snapshot.thread.latestTurn?.sourceProposedPlan).toEqual(sourceProposedPlan);
  });

  it("rejects an explicit null source even when the checkpoint is valid", () => {
    const wire = savedPlanSnapshot();
    Object.assign(wire.snapshot.thread.latestTurn, { sourceProposedPlan: null });
    expect(() => decodeThreadStreamItem(wire)).toThrow(
      '["snapshot"]["thread"]["latestTurn"]["sourceProposedPlan"]',
    );
  });

  it("rejects a completed checkpoint even when the optional source is absent", () => {
    const wire = savedPlanSnapshot();
    wire.snapshot.thread.checkpoints[0]!.status = "completed";
    expect(() => decodeThreadStreamItem(wire)).toThrow(
      '["snapshot"]["thread"]["checkpoints"][0]["status"]',
    );
  });
});
