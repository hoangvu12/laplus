import type { VcsStatusResult } from "@t3tools/contracts";
import { describe, expect, it } from "vite-plus/test";

import { resolveThreadPr } from "./ThreadStatusIndicators";

function status(overrides: Partial<VcsStatusResult> = {}): VcsStatusResult {
  return {
    isRepo: true,
    hasPrimaryRemote: true,
    isDefaultRef: false,
    refName: "feature/current",
    hasWorkingTreeChanges: false,
    workingTree: { files: [], insertions: 0, deletions: 0 },
    hasUpstream: true,
    aheadCount: 0,
    behindCount: 0,
    pr: {
      number: 42,
      title: "PR branch",
      url: "https://github.com/pingdotgg/t3code/pull/42",
      baseRef: "main",
      headRef: "feature/current",
      state: "open",
    },
    ...overrides,
  };
}

describe("resolveThreadPr", () => {
  it("keeps local-checkout PR indicators scoped to the stored thread branch", () => {
    expect(
      resolveThreadPr({
        threadBranch: "feature/other",
        gitStatus: status(),
      }),
    ).toBeNull();
  });

  it("hides PR indicators when a dedicated worktree has switched away from the thread branch", () => {
    expect(
      resolveThreadPr({
        threadBranch: "stack/base",
        gitStatus: status(),
      }),
    ).toBeNull();
  });

  it("hides PR indicators when thread branch metadata is missing", () => {
    expect(
      resolveThreadPr({
        threadBranch: null,
        gitStatus: status(),
      }),
    ).toBeNull();
  });

  it("shows the PR when the live checkout matches the stored thread branch", () => {
    const gitStatus = status();

    expect(
      resolveThreadPr({
        threadBranch: "feature/current",
        gitStatus,
      }),
    ).toBe(gitStatus.pr);
  });
});
