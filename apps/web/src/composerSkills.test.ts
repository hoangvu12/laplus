import { describe, expect, it } from "vite-plus/test";

import {
  encodeComposerSkills,
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
});
