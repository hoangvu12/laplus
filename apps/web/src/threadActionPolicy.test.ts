import { describe, expect, it } from "vite-plus/test";
import { threadActionPolicy } from "./threadActionPolicy";

describe("threadActionPolicy", () => {
  it("offers the same pin action to every surface when supported", () => {
    expect(threadActionPolicy({ pinningSupported: true, pinned: false })).toMatchObject({
      pinAction: { id: "pin", label: "Pin thread" },
    });
    expect(threadActionPolicy({ pinningSupported: true, pinned: true })).toMatchObject({
      pinAction: { id: "unpin", label: "Unpin thread" },
    });
  });

  it("omits pinning on older servers", () => {
    expect(threadActionPolicy({ pinningSupported: false, pinned: false }).pinAction).toBeNull();
  });

  it("offers regeneration for eligible threads and exposes its pending state", () => {
    expect(
      threadActionPolicy({
        pinningSupported: false,
        pinned: false,
        titleRegenerationSupported: true,
        titleRegenerationPending: false,
      }).regenerateTitle,
    ).toEqual({ id: "regenerate-title", label: "Regenerate title", disabled: false });
    expect(
      threadActionPolicy({
        pinningSupported: false,
        pinned: false,
        titleRegenerationSupported: true,
        titleRegenerationPending: true,
      }).regenerateTitle,
    ).toEqual({ id: "regenerate-title", label: "Regenerating…", disabled: true });
    expect(
      threadActionPolicy({
        pinningSupported: false,
        pinned: false,
        titleRegenerationSupported: true,
        archived: true,
      }).regenerateTitle,
    ).toBeNull();
  });

  it("owns lifecycle and destructive menu decisions", () => {
    expect(
      threadActionPolicy({
        pinningSupported: true,
        pinned: true,
        snoozed: true,
        snoozeSupported: true,
      }),
    ).toMatchObject({
      rename: true,
      copy: true,
      lifecycleAction: { id: "unsnooze", label: "Wake thread" },
      destructiveAction: { id: "archive", label: "Archive thread" },
    });
    expect(
      threadActionPolicy({ pinningSupported: false, pinned: false, archived: true })
        .destructiveAction.id,
    ).toBe("delete");
  });
});
