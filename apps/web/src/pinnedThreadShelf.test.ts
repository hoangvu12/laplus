import { describe, expect, it } from "vite-plus/test";
import { resolveThreadShelfSection } from "./pinnedThreadShelf";

describe("resolveThreadShelfSection", () => {
  it("temporarily hides a pin while snoozed and restores it on wake", () => {
    expect(resolveThreadShelfSection({ pinned: true, snoozed: true, settled: false })).toBe(
      "snoozed",
    );
    expect(resolveThreadShelfSection({ pinned: true, snoozed: false, settled: false })).toBe(
      "pinned",
    );
  });

  it("gives visible pinning precedence over settlement", () => {
    expect(resolveThreadShelfSection({ pinned: true, snoozed: false, settled: true })).toBe(
      "pinned",
    );
    expect(resolveThreadShelfSection({ pinned: false, snoozed: false, settled: true })).toBe(
      "settled",
    );
  });
});
