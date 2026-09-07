import { describe, expect, it } from "vite-plus/test";
import type { OrchestrationThreadActivity } from "@t3tools/contracts";

import {
  encodeComposerSkills,
  resolveMimirSkillPreparation,
  selectMimirSkillCatalog,
  reconcileComposerSkills,
  selectedComposerSkill,
} from "./composerSkills";

const skill = { name: "review", description: "Review", path: "/skills/review", enabled: true };

describe("composer skills", () => {
  it("retains structured identity through edits around a selected chip", () => {
    const selected = selectedComposerSkill(skill, 3);
    expect(reconcileComposerSkills("go $review now", "please go $review now", [selected])).toEqual([
      { ...selected, start: 10, end: 17 },
    ]);
    expect(reconcileComposerSkills("go $review now", "go $renamed now", [selected])).toEqual([]);
  });

  it("encodes UTF-8 byte offsets in the final provider prompt", () => {
    const selected = selectedComposerSkill(skill, 3);
    expect(encodeComposerSkills("🙂\n\ngo $review", "go $review", [selected])).toEqual([
      {
        name: "review",
        path: "/skills/review",
        visibleText: "$review",
        textRange: { start: 9, end: 16 },
      },
    ]);
  });

  it("selects only the newest catalog for the requested provider and canonical cwd", () => {
    const catalog = (providerInstanceId: string, cwd: string, sdkSessionId: string, name: string) =>
      ({
        kind: "provider.skills",
        payload: {
          scope: { providerInstanceId, cwd, sdkSessionId },
          skills: [{ ...skill, name }],
        },
      }) as unknown as OrchestrationThreadActivity;
    const activities = [
      catalog("mimir-a", "/work", "stale", "stale"),
      catalog("mimir-b", "/work", "other-provider", "wrong"),
      catalog("mimir-a", "/other", "other-cwd", "wrong"),
      catalog("mimir-a", "/work", "current", "current"),
    ];

    expect(selectMimirSkillCatalog(activities, "mimir-a", "/work")?.skills[0]?.name).toBe(
      "current",
    );
    expect(selectMimirSkillCatalog(activities, "mimir-a", "/missing")).toBeNull();
  });

  it("keeps preparation pending through acknowledgement until a new matching catalog arrives", () => {
    const oldCatalog = {
      id: "old-catalog",
      kind: "provider.skills",
      tone: "info",
      summary: "Mimir session skills",
      payload: {
        scope: { providerInstanceId: "mimir-a", cwd: "/work", sdkSessionId: "session-a" },
        skills: [skill],
      },
    } as unknown as OrchestrationThreadActivity;
    const pending = {
      providerInstanceId: "mimir-a",
      cwd: "/work",
      previousActivityIds: new Set([oldCatalog.id]),
    };

    expect(resolveMimirSkillPreparation([oldCatalog], pending)).toEqual({ status: "pending" });
    const wrongScope = {
      ...oldCatalog,
      id: "wrong-scope",
      payload: {
        scope: { providerInstanceId: "mimir-b", cwd: "/work", sdkSessionId: "session-b" },
        skills: [skill],
      },
    } as OrchestrationThreadActivity;
    expect(resolveMimirSkillPreparation([oldCatalog, wrongScope], pending)).toEqual({
      status: "pending",
    });
    const refreshed = { ...oldCatalog, id: "refreshed" } as OrchestrationThreadActivity;
    expect(resolveMimirSkillPreparation([oldCatalog, refreshed], pending)).toEqual({
      status: "ready",
    });
  });

  it("ends preparation on a new asynchronous catalog or session failure", () => {
    const pending = {
      providerInstanceId: "mimir-a",
      cwd: "/work",
      previousActivityIds: new Set<string>(),
    };
    const failure = (kind: string) =>
      ({
        id: `failed-${kind}`,
        kind,
        tone: "error",
        summary: "Preparation failed",
        payload: { detail: "SDK unavailable" },
      }) as unknown as OrchestrationThreadActivity;

    expect(resolveMimirSkillPreparation([failure("provider.skills")], pending)).toEqual({
      status: "failed",
      message: "SDK unavailable",
    });
    expect(resolveMimirSkillPreparation([failure("session.failed")], pending)).toEqual({
      status: "failed",
      message: "SDK unavailable",
    });
  });
});
