// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import { ComposerPrimaryActions } from "./ComposerPrimaryActions";
import { ComposerTaskProgress } from "./ComposerTaskProgress";

afterEach(cleanup);

describe("native session actions", () => {
  it("summarizes native tasks and retains failures without spinning after a turn stops", () => {
    const steps = [
      { step: "Inspect", status: "completed" as const },
      { step: "Implement", status: "inProgress" as const },
      { step: "Verify", status: "failed" as const },
      { step: "Publish", status: "cancelled" as const },
    ];
    const { rerender } = render(
      <ComposerTaskProgress plan={{ createdAt: "now", turnId: null, steps }} running />,
    );
    expect(screen.getByText("1/4 completed")).toBeTruthy();
    expect(screen.getAllByLabelText("In progress")[0]?.classList.contains("animate-spin")).toBe(
      true,
    );
    expect(screen.getByLabelText("failed")).toBeTruthy();
    expect(screen.getByLabelText("cancelled")).toBeTruthy();
    rerender(
      <ComposerTaskProgress plan={{ createdAt: "now", turnId: null, steps }} running={false} />,
    );
    expect(screen.getAllByLabelText("Paused")[0]?.classList.contains("animate-spin")).toBe(false);
  });

  it("keeps active steering separate from queued follow-up submission and stop", () => {
    const onSteer = vi.fn();
    const onInterrupt = vi.fn();
    const submit = vi.fn((event) => event.preventDefault());
    render(
      <form onSubmit={submit}>
        <ComposerPrimaryActions
          compact={false}
          pendingAction={null}
          isRunning
          showPlanFollowUpPrompt={false}
          promptHasText
          isSendBusy={false}
          isConnecting={false}
          isEnvironmentUnavailable={false}
          isPreparingWorktree={false}
          hasSendableContent
          onPreviousPendingQuestion={() => {}}
          onInterrupt={onInterrupt}
          onImplementPlanInNewThread={() => {}}
          runningActions={{ onSteer, canSteer: true }}
        />
      </form>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Steer" }));
    expect(onSteer).toHaveBeenCalledOnce();
    expect(submit).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Queue follow-up" }));
    expect(submit).toHaveBeenCalledOnce();
    expect(onSteer).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole("button", { name: "Stop generation" }));
    expect(onInterrupt).toHaveBeenCalledOnce();
  });

  it("disables a blocked command's queue and steering actions without disabling stop", () => {
    const onSteer = vi.fn();
    const onInterrupt = vi.fn();
    const submit = vi.fn((event) => event.preventDefault());
    render(
      <form onSubmit={submit}>
        <ComposerPrimaryActions
          compact={false}
          pendingAction={null}
          isRunning
          showPlanFollowUpPrompt={false}
          promptHasText
          isSendBusy={false}
          isConnecting={false}
          isEnvironmentUnavailable={false}
          isPreparingWorktree={false}
          hasSendableContent={false}
          onPreviousPendingQuestion={() => {}}
          onInterrupt={onInterrupt}
          onImplementPlanInNewThread={() => {}}
          runningActions={{ onSteer, canSteer: false }}
        />
      </form>,
    );
    for (const name of ["Steer", "Queue follow-up"]) {
      const button = screen.getByRole("button", { name }) as HTMLButtonElement;
      expect(button.disabled).toBe(true);
      fireEvent.click(button);
    }
    expect(onSteer).not.toHaveBeenCalled();
    expect(submit).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Stop generation" }));
    expect(onInterrupt).toHaveBeenCalledOnce();
  });

  it("disables text-only steering when the draft contains attachments", () => {
    const onSteer = vi.fn();
    render(
      <ComposerPrimaryActions
        compact
        pendingAction={null}
        isRunning
        showPlanFollowUpPrompt={false}
        promptHasText
        isSendBusy={false}
        isConnecting={false}
        isEnvironmentUnavailable={false}
        isPreparingWorktree={false}
        hasSendableContent
        onPreviousPendingQuestion={() => {}}
        onInterrupt={() => {}}
        onImplementPlanInNewThread={() => {}}
        runningActions={{ onSteer, canSteer: false }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Steer" }));
    expect(onSteer).not.toHaveBeenCalled();
    expect(
      (screen.getByRole("button", { name: "Queue follow-up" }) as HTMLButtonElement).disabled,
    ).toBe(false);
  });
});
