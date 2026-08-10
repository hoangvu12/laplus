import { EnvironmentId } from "@t3tools/contracts";
import { describe, expect, it } from "vite-plus/test";

import {
  chatHeaderActionTriggerLabel,
  resolveChatHeaderThreadActions,
  shouldShowOpenInPicker,
} from "./ChatHeader";

describe("shouldShowOpenInPicker", () => {
  const primaryEnvironmentId = EnvironmentId.make("environment-primary");

  it("shows the picker for projects in the primary environment", () => {
    expect(
      shouldShowOpenInPicker({
        activeProjectName: "codething-mvp",
        activeThreadEnvironmentId: primaryEnvironmentId,
        primaryEnvironmentId,
      }),
    ).toBe(true);
  });

  it("hides the picker when there is no primary environment", () => {
    expect(
      shouldShowOpenInPicker({
        activeProjectName: "codething-mvp",
        activeThreadEnvironmentId: EnvironmentId.make("environment-remote"),
        primaryEnvironmentId: null,
      }),
    ).toBe(false);
  });

  it("hides the picker for remote environments", () => {
    expect(
      shouldShowOpenInPicker({
        activeProjectName: "codething-mvp",
        activeThreadEnvironmentId: EnvironmentId.make("environment-remote"),
        primaryEnvironmentId,
      }),
    ).toBe(false);
  });

  it("hides the picker when there is no active project", () => {
    expect(
      shouldShowOpenInPicker({
        activeProjectName: undefined,
        activeThreadEnvironmentId: primaryEnvironmentId,
        primaryEnvironmentId,
      }),
    ).toBe(false);
  });
});

describe("resolveChatHeaderThreadActions", () => {
  it("exposes an accessible title-menu label", () => {
    expect(chatHeaderActionTriggerLabel("Fix release")).toBe("Fix release actions");
  });
  it("uses the shared policy for the complete supported action menu", () => {
    expect(
      resolveChatHeaderThreadActions({
        pinningSupported: true,
        pinned: false,
        settled: false,
        snoozed: false,
        settlementSupported: true,
        snoozeSupported: false,
      }),
    ).toEqual(["rename", "copy", "pin", "settle", "archive"]);
  });

  it("hides unsupported pinning and reflects wake/unpin state", () => {
    expect(
      resolveChatHeaderThreadActions({
        pinningSupported: false,
        pinned: true,
        snoozed: true,
        snoozeSupported: true,
      }),
    ).toEqual(["rename", "copy", "unsnooze", "archive"]);
  });
});
