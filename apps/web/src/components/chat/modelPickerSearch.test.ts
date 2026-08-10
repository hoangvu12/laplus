import { describe, expect, it } from "vite-plus/test";

import { buildModelPickerSearchText, scoreModelPickerSearch } from "./modelPickerSearch";

describe("buildModelPickerSearchText", () => {
  it("builds provider-agnostic search text from generic fields", () => {
    expect(
      buildModelPickerSearchText({
        driverKind: "opencode",
        providerDisplayName: "opencode",
        name: "Claude Opus 4.7",
        subProvider: "GitHub Copilot",
      }),
    ).toBe("claude opus 4.7 github copilot opencode opencode");
  });
});

describe("scoreModelPickerSearch", () => {
  it("matches typo-tolerant multi-token queries", () => {
    expect(
      scoreModelPickerSearch(
        {
          driverKind: "opencode",
          providerDisplayName: "opencode",
          name: "Claude Opus 4.7",
          subProvider: "GitHub Copilot",
        },
        "coplt op",
      ),
    ).not.toBeNull();
  });

  it("rejects results when any query token does not match", () => {
    expect(
      scoreModelPickerSearch(
        {
          driverKind: "codex",
          providerDisplayName: "codex",
          name: "GPT-5 Codex",
        },
        "coplt op",
      ),
    ).toBeNull();
  });

  it("ranks exact token matches ahead of fuzzier matches", () => {
    const exactScore = scoreModelPickerSearch(
      {
        driverKind: "opencode",
        providerDisplayName: "opencode",
        name: "Claude Opus 4.7",
        subProvider: "GitHub Copilot",
      },
      "copilot opus",
    );
    const fuzzyScore = scoreModelPickerSearch(
      {
        driverKind: "opencode",
        providerDisplayName: "opencode",
        name: "Claude Opus 4.7",
        subProvider: "GitHub Copilot",
      },
      "coplt op",
    );

    expect(exactScore).not.toBeNull();
    expect(fuzzyScore).not.toBeNull();
    expect(exactScore!).toBeLessThan(fuzzyScore!);
  });

  it("matches an upstream provider id that only the slug carries", () => {
    // OpenCode fronts a provider per slug segment, and the id there is the one
    // thing a person reads off `opencode auth list`. Before the slug was
    // indexed, typing it found nothing while the provider's whole catalogue sat
    // in the list.
    const model = {
      driverKind: "opencode",
      providerDisplayName: "OpenCode",
      name: "Qwen3.5 27B",
      slug: "siliconflow/Qwen/Qwen3.5-27B",
    };

    expect(scoreModelPickerSearch(model, "siliconflow")).not.toBeNull();
    expect(scoreModelPickerSearch(model, "siliconflow qwen3.5")).not.toBeNull();
    expect(scoreModelPickerSearch(model, "dashscope")).toBeNull();
  });

  it("ranks a name match ahead of the same query matching only a slug", () => {
    const byName = scoreModelPickerSearch(
      { driverKind: "opencode", providerDisplayName: "OpenCode", name: "Qwen3.5 27B", slug: "x/y" },
      "qwen3.5",
    );
    const bySlug = scoreModelPickerSearch(
      {
        driverKind: "opencode",
        providerDisplayName: "OpenCode",
        name: "Some Other Model",
        slug: "siliconflow/Qwen/Qwen3.5-27B",
      },
      "qwen3.5",
    );

    expect(byName).not.toBeNull();
    expect(bySlug).not.toBeNull();
    expect(byName!).toBeLessThan(bySlug!);
  });

  it("gives favorite models a strong enough ranking boost for partial queries", () => {
    const favoriteScore = scoreModelPickerSearch(
      {
        driverKind: "claudeAgent",
        providerDisplayName: "Claude",
        name: "Claude Opus 4.7",
        isFavorite: true,
      },
      "opu",
    );
    const nonFavoriteScore = scoreModelPickerSearch(
      {
        driverKind: "cursor",
        providerDisplayName: "Cursor",
        name: "Opus 4.5",
      },
      "opu",
    );

    expect(favoriteScore).not.toBeNull();
    expect(nonFavoriteScore).not.toBeNull();
    expect(favoriteScore!).toBeLessThan(nonFavoriteScore!);
  });

  it("does not let the favorite boost outrank clearly better textual matches", () => {
    const favoriteScore = scoreModelPickerSearch(
      {
        driverKind: "claudeAgent",
        providerDisplayName: "Claude",
        name: "Claude Opus 4.7",
        isFavorite: true,
      },
      "opus 4.7",
    );
    const nonFavoriteExactScore = scoreModelPickerSearch(
      {
        driverKind: "cursor",
        providerDisplayName: "Cursor",
        name: "Opus 4.7",
      },
      "opus 4.7",
    );

    expect(favoriteScore).not.toBeNull();
    expect(nonFavoriteExactScore).not.toBeNull();
    expect(nonFavoriteExactScore!).toBeLessThan(favoriteScore!);
  });

  it("matches a custom instance's display name against its models", () => {
    expect(
      scoreModelPickerSearch(
        {
          driverKind: "codex",
          providerDisplayName: "Codex Personal",
          name: "GPT-5 Codex",
        },
        "personal",
      ),
    ).not.toBeNull();
  });
});
