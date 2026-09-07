// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { scopeThreadRef } from "@t3tools/client-runtime/environment";
import { EnvironmentId, OrchestrationProposedPlan, ThreadId, TurnId } from "@t3tools/contracts";
import { Schema } from "effect";
import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { useComposerDraftStore } from "../composerDraftStore";
import {
  findLatestProposedPlan,
  getMimirPlanDecisionAvailability,
  hasActionableProposedPlan,
  isLatestTurnSettled,
} from "../session-logic";
import { clearSubmittedSteerText, runMimirPlanDecision } from "./ChatView.logic";
import { ComposerPlanFollowUpBanner } from "./chat/ComposerPlanFollowUpBanner";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

const decodePlan = Schema.decodeUnknownSync(OrchestrationProposedPlan);
const target = scopeThreadRef(EnvironmentId.make("local"), ThreadId.make("mimir-thread"));
const idle = { status: "ready", activeTurnId: null } as const;
const savedPlan = {
  id: "actual-sdk-plan-42",
  turnId: null,
  planMarkdown: "# Durable saved plan",
  status: "review-pending",
  implementedAt: null,
  implementationThreadId: null,
  createdAt: "2026-09-06T00:00:00.000Z",
  updatedAt: "2026-09-06T00:00:00.000Z",
};

beforeEach(() => {
  useComposerDraftStore.setState({ draftsByThreadKey: {}, draftThreadsByThreadKey: {} });
});
afterEach(cleanup);

// Decode the same durable plan payload the subscription/reload path receives.
function reopenPlan(payload: unknown) {
  return findLatestProposedPlan([decodePlan(JSON.parse(JSON.stringify(payload)))], null)!;
}

describe("ChatView Mimir lifecycle", () => {
  it("offers later Implement for the same SDK ID after save-and-stop and reload, but not Save again", () => {
    const onDecide = vi.fn();
    const submit = vi.fn((event) => event.preventDefault());
    const plan = reopenPlan(savedPlan);
    const { rerender } = render(
      <form onSubmit={submit}>
        <ComposerPlanFollowUpBanner
          planTitle="Durable saved plan"
          decision={{
            planId: plan.id,
            ...getMimirPlanDecisionAvailability(plan, idle, false),
            onDecide,
          }}
        />
      </form>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Save and stop" }));
    expect(onDecide).toHaveBeenLastCalledWith("actual-sdk-plan-42", "save-and-stop");
    expect(submit).not.toHaveBeenCalled();

    const reloaded = reopenPlan({
      ...savedPlan,
      status: "saved-stopped",
      decision: "save-and-stop",
    });
    expect(reloaded.status).toBe("saved-stopped");
    expect(hasActionableProposedPlan(reloaded)).toBe(true);
    expect(isLatestTurnSettled(null, idle)).toBe(false);
    rerender(
      <form onSubmit={submit}>
        <ComposerPlanFollowUpBanner
          planTitle="Durable saved plan"
          decision={{
            planId: reloaded.id,
            ...getMimirPlanDecisionAvailability(reloaded, idle, false),
            onDecide,
          }}
        />
      </form>,
    );
    expect(
      (screen.getByRole("button", { name: "Save and stop" }) as HTMLButtonElement).disabled,
    ).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Save and stop" }));
    expect(onDecide).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Implement" }));
    expect(onDecide).toHaveBeenLastCalledWith("actual-sdk-plan-42", "implement");
    expect(submit).not.toHaveBeenCalled();
    expect(
      hasActionableProposedPlan(
        reopenPlan({ ...savedPlan, status: "accepted", decision: "implement" }),
      ),
    ).toBe(false);
  });

  it("disables old-plan decisions during refinement and re-enables them on idle without historical turns", () => {
    const plan = reopenPlan(savedPlan);
    const onDecide = vi.fn();
    const running = { status: "running", activeTurnId: TurnId.make("refinement") } as const;
    const availability = getMimirPlanDecisionAvailability(plan, running, false);
    expect(availability).toEqual({ canImplement: false, canSaveAndStop: false });
    const { rerender } = render(
      <ComposerPlanFollowUpBanner
        planTitle="Prior saved plan"
        decision={{ planId: plan.id, ...availability, onDecide }}
      />,
    );
    for (const name of ["Implement", "Save and stop"]) {
      expect((screen.getByRole("button", { name }) as HTMLButtonElement).disabled).toBe(true);
      fireEvent.click(screen.getByRole("button", { name }));
    }
    expect(onDecide).not.toHaveBeenCalled();
    rerender(
      <ComposerPlanFollowUpBanner
        planTitle="Prior saved plan"
        decision={{
          planId: plan.id,
          ...getMimirPlanDecisionAvailability(plan, idle, false),
          onDecide,
        }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Implement" }));
    expect(onDecide).toHaveBeenCalledWith(plan.id, "implement");
    expect(getMimirPlanDecisionAvailability(plan, idle, true).canImplement).toBe(false);
    expect(
      getMimirPlanDecisionAvailability(
        plan,
        { ...idle, activeTurnId: TurnId.make("starting") },
        false,
      ).canImplement,
    ).toBe(false);
  });

  it.each(["drafting", "accepted", "implementing", "completed", "abandoned"])(
    "does not offer decisions for a %s plan even while the session is idle",
    (status) => {
      const plan = reopenPlan({ ...savedPlan, status });
      expect(hasActionableProposedPlan(plan)).toBe(false);
      expect(getMimirPlanDecisionAvailability(plan, idle, false)).toEqual({
        canImplement: false,
        canSaveAndStop: false,
      });
    },
  );

  it.each(["implement", "save-and-stop"])(
    "the next ordinary send uses Build after successful %s",
    async (decision) => {
      const store = useComposerDraftStore.getState();
      store.setInteractionMode(target, "plan");
      const response = deferred<boolean>();
      const dispatch = vi.fn((_planId: string, _decision: string) => response.promise);
      const pending = runMimirPlanDecision(target, () => dispatch(savedPlan.id, decision));
      store.setPrompt(target, "Next ordinary message");
      response.resolve(true);
      await pending;
      expect(dispatch).toHaveBeenCalledWith(savedPlan.id, decision);
      const draft = store.getComposerDraft(target);
      // ChatView's next-send resolution is draft override ?? server interaction mode.
      // The local override must be Build even before the server mode event arrives.
      expect(draft?.interactionMode ?? "plan").toBe("default");
      expect(draft?.prompt).toBe("Next ordinary message");
    },
  );

  it("preserves a newer mode choice, including changing away and back during a deferred decision", async () => {
    const store = useComposerDraftStore.getState();
    store.setInteractionMode(target, "plan");
    const response = deferred<boolean>();
    const pending = runMimirPlanDecision(target, () => response.promise);
    store.setInteractionMode(target, "default");
    store.setInteractionMode(target, "plan");
    response.resolve(true);
    await pending;
    expect(store.getComposerDraft(target)?.interactionMode).toBe("plan");
  });

  it("does not select Build for rejected or interrupted decisions", async () => {
    const store = useComposerDraftStore.getState();
    store.setInteractionMode(target, "plan");
    await runMimirPlanDecision(target, () => Promise.resolve(false));
    expect(store.getComposerDraft(target)?.interactionMode).toBe("plan");
    await expect(
      runMimirPlanDecision(target, () => Promise.reject(new Error("disconnected"))),
    ).rejects.toThrow("disconnected");
    expect(store.getComposerDraft(target)?.interactionMode).toBe("plan");
  });

  it("retains an image pasted while steering awaits acknowledgement and clears only submitted text", async () => {
    const store = useComposerDraftStore.getState();
    const submittedText = "Focus on tests";
    store.setPrompt(target, submittedText);
    const response = deferred<void>();
    const pending = response.promise.then(() => clearSubmittedSteerText(target, submittedText));
    const image = {
      type: "image" as const,
      id: "new-image",
      name: "new.png",
      mimeType: "image/png",
      sizeBytes: 3,
      previewUrl: "blob:new-image",
      file: new File(["png"], "new.png", { type: "image/png" }),
    };
    store.addImage(target, image);
    response.resolve();
    expect(await pending).toBe(true);
    expect(store.getComposerDraft(target)?.prompt).toBe("");
    expect(store.getComposerDraft(target)?.images).toEqual([image]);
  });

  it("preserves newly attached terminal context and its placeholder during deferred steering", async () => {
    const store = useComposerDraftStore.getState();
    store.setPrompt(target, "Focus on tests");
    const response = deferred<void>();
    const pending = response.promise.then(() => clearSubmittedSteerText(target, "Focus on tests"));
    store.setTerminalContexts(target, [
      {
        id: "new-context",
        threadId: target.threadId,
        terminalId: "terminal",
        terminalLabel: "Terminal",
        lineStart: 1,
        lineEnd: 1,
        text: "new terminal output",
        createdAt: savedPlan.createdAt,
      },
    ]);
    const draftBeforeResponse = store.getComposerDraft(target);
    response.resolve();
    await pending;
    expect(store.getComposerDraft(target)).toEqual(draftBeforeResponse);
    expect(store.getComposerDraft(target)?.terminalContexts[0]?.text).toBe("new terminal output");
  });

  it("does not clear text edited while steering awaits acknowledgement", () => {
    const store = useComposerDraftStore.getState();
    store.setPrompt(target, "Focus on tests ");
    expect(clearSubmittedSteerText(target, "Focus on tests")).toBe(false);
    expect(store.getComposerDraft(target)?.prompt).toBe("Focus on tests ");
  });
});
